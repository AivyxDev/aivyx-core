//! Audit log aggregation for observability metrics.
//!
//! Provides [`compute_summary`] and [`compute_timeline`] functions that scan
//! audit entries to produce cost, token, and event counters. The `cost_fn`
//! closure decouples this module from `aivyx-config`'s pricing data.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::event::AuditEvent;
use crate::log::AuditEntry;

/// Aggregated metrics over a time range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSummary {
    /// Start of the time range (inclusive).
    pub from: DateTime<Utc>,
    /// End of the time range (inclusive).
    pub to: DateTime<Utc>,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Total input tokens across all LLM requests.
    pub total_input_tokens: u64,
    /// Total output tokens across all LLM requests.
    pub total_output_tokens: u64,
    /// Number of LLM responses received.
    pub llm_requests: u64,
    /// Number of agent turns completed.
    pub agent_turns: u64,
    /// Number of tool executions.
    pub tool_executions: u64,
    /// Number of tool denials.
    pub tool_denials: u64,
    /// Number of distinct agents that had turns.
    pub unique_agents: usize,
    /// Number of HTTP authentication failures.
    pub auth_failures: u64,
    /// Number of rate limit exceeded events.
    pub rate_limit_events: u64,
}

/// A single hourly bucket in the metrics timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineBucket {
    /// Start of the hour (UTC).
    pub hour: DateTime<Utc>,
    /// Estimated cost in USD for this hour.
    pub cost_usd: f64,
    /// Input tokens in this hour.
    pub input_tokens: u64,
    /// Output tokens in this hour.
    pub output_tokens: u64,
    /// LLM responses received in this hour.
    pub llm_requests: u64,
    /// Agent turns completed in this hour.
    pub agent_turns: u64,
    /// Tool executions in this hour.
    pub tool_executions: u64,
    /// Errors (tool denials + auth failures + rate limits) in this hour.
    pub errors: u64,
}

/// Compute an aggregated summary over the given entries within `[from, to]`.
///
/// `cost_fn(input_tokens, output_tokens, provider) -> USD` maps token counts
/// to dollar amounts. Entries outside the time range are ignored.
pub fn compute_summary(
    entries: &[AuditEntry],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    cost_fn: &dyn Fn(u32, u32, &str) -> f64,
) -> MetricsSummary {
    let mut summary = MetricsSummary {
        from,
        to,
        total_cost_usd: 0.0,
        total_input_tokens: 0,
        total_output_tokens: 0,
        llm_requests: 0,
        agent_turns: 0,
        tool_executions: 0,
        tool_denials: 0,
        unique_agents: 0,
        auth_failures: 0,
        rate_limit_events: 0,
    };

    let mut unique_agents: HashSet<String> = HashSet::new();

    for entry in entries {
        let ts = match parse_entry_timestamp(entry) {
            Some(t) => t,
            None => continue,
        };
        if ts < from || ts > to {
            continue;
        }

        match &entry.event {
            AuditEvent::LlmResponseReceived {
                agent_id: _,
                provider,
                input_tokens,
                output_tokens,
                stop_reason: _,
            } => {
                summary.llm_requests += 1;
                summary.total_input_tokens += *input_tokens as u64;
                summary.total_output_tokens += *output_tokens as u64;
                summary.total_cost_usd += cost_fn(*input_tokens, *output_tokens, provider);
            }
            AuditEvent::AgentTurnCompleted { agent_id, .. } => {
                summary.agent_turns += 1;
                unique_agents.insert(agent_id.to_string());
            }
            AuditEvent::ToolExecuted { .. } => {
                summary.tool_executions += 1;
            }
            AuditEvent::ToolDenied { .. } => {
                summary.tool_denials += 1;
            }
            AuditEvent::HttpAuthFailed { .. } => {
                summary.auth_failures += 1;
            }
            AuditEvent::RateLimitExceeded { .. } => {
                summary.rate_limit_events += 1;
            }
            _ => {}
        }
    }

    summary.unique_agents = unique_agents.len();
    summary
}

/// Compute hourly timeline buckets over the given entries within `[from, to]`.
///
/// Returns one bucket per hour. Partial hours at the boundaries are included.
pub fn compute_timeline(
    entries: &[AuditEntry],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    cost_fn: &dyn Fn(u32, u32, &str) -> f64,
) -> Vec<TimelineBucket> {
    let total_hours = (to - from).num_hours().max(1) as usize;
    // Cap at a reasonable maximum (e.g., 8760 = 1 year of hours)
    let bucket_count = total_hours.min(8760);

    let mut buckets: Vec<TimelineBucket> = (0..bucket_count)
        .map(|i| {
            let hour = from + chrono::Duration::hours(i as i64);
            TimelineBucket {
                hour,
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                llm_requests: 0,
                agent_turns: 0,
                tool_executions: 0,
                errors: 0,
            }
        })
        .collect();

    if buckets.is_empty() {
        return buckets;
    }

    for entry in entries {
        let ts = match parse_entry_timestamp(entry) {
            Some(t) => t,
            None => continue,
        };
        if ts < from || ts > to {
            continue;
        }

        let idx = (ts - from).num_hours().max(0) as usize;
        let idx = idx.min(bucket_count.saturating_sub(1));

        match &entry.event {
            AuditEvent::LlmResponseReceived {
                provider,
                input_tokens,
                output_tokens,
                ..
            } => {
                buckets[idx].llm_requests += 1;
                buckets[idx].input_tokens += *input_tokens as u64;
                buckets[idx].output_tokens += *output_tokens as u64;
                buckets[idx].cost_usd += cost_fn(*input_tokens, *output_tokens, provider);
            }
            AuditEvent::AgentTurnCompleted { .. } => {
                buckets[idx].agent_turns += 1;
            }
            AuditEvent::ToolExecuted { .. } => {
                buckets[idx].tool_executions += 1;
            }
            AuditEvent::ToolDenied { .. } => {
                buckets[idx].errors += 1;
            }
            AuditEvent::HttpAuthFailed { .. } => {
                buckets[idx].errors += 1;
            }
            AuditEvent::RateLimitExceeded { .. } => {
                buckets[idx].errors += 1;
            }
            _ => {}
        }
    }

    buckets
}

/// Parse the RFC 3339 timestamp from an audit entry.
fn parse_entry_timestamp(entry: &AuditEntry) -> Option<DateTime<Utc>> {
    entry.timestamp.parse::<DateTime<Utc>>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AuditEvent;
    use aivyx_core::AgentId;
    use chrono::TimeZone;

    fn make_entry(seq: u64, ts: DateTime<Utc>, event: AuditEvent) -> AuditEntry {
        AuditEntry {
            sequence_number: seq,
            timestamp: ts.to_rfc3339(),
            event,
            hmac: String::new(),
        }
    }

    fn zero_cost(_input: u32, _output: u32, _provider: &str) -> f64 {
        0.0
    }

    fn simple_cost(input: u32, output: u32, _provider: &str) -> f64 {
        (input as f64 * 0.000003) + (output as f64 * 0.000015)
    }

    #[test]
    fn empty_entries_produce_zero_summary() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
        let summary = compute_summary(&[], from, to, &zero_cost);

        assert_eq!(summary.llm_requests, 0);
        assert_eq!(summary.agent_turns, 0);
        assert_eq!(summary.tool_executions, 0);
        assert_eq!(summary.unique_agents, 0);
        assert_eq!(summary.total_cost_usd, 0.0);
    }

    #[test]
    fn summary_counts_llm_and_tools() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();
        let agent_id = AgentId::new();

        let entries = vec![
            make_entry(
                0,
                ts,
                AuditEvent::LlmResponseReceived {
                    agent_id: agent_id.clone(),
                    provider: "claude".into(),
                    input_tokens: 1000,
                    output_tokens: 500,
                    stop_reason: "end_turn".into(),
                },
            ),
            make_entry(
                1,
                ts,
                AuditEvent::ToolExecuted {
                    tool_id: aivyx_core::ToolId::new(),
                    agent_id: agent_id.clone(),
                    action: "file_read".into(),
                    result_summary: "ok".into(),
                },
            ),
            make_entry(
                2,
                ts,
                AuditEvent::AgentTurnCompleted {
                    agent_id,
                    session_id: aivyx_core::SessionId::new(),
                    tool_calls_made: 1,
                    tokens_used: 1500,
                },
            ),
        ];

        let summary = compute_summary(&entries, from, to, &simple_cost);

        assert_eq!(summary.llm_requests, 1);
        assert_eq!(summary.total_input_tokens, 1000);
        assert_eq!(summary.total_output_tokens, 500);
        assert_eq!(summary.tool_executions, 1);
        assert_eq!(summary.agent_turns, 1);
        assert_eq!(summary.unique_agents, 1);
        assert!(summary.total_cost_usd > 0.0);
    }

    #[test]
    fn summary_filters_by_time_range() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();
        let inside = Utc.with_ymd_and_hms(2026, 3, 1, 6, 0, 0).unwrap();
        let outside = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();

        let entries = vec![
            make_entry(
                0,
                inside,
                AuditEvent::ToolExecuted {
                    tool_id: aivyx_core::ToolId::new(),
                    agent_id: AgentId::new(),
                    action: "file_read".into(),
                    result_summary: "ok".into(),
                },
            ),
            make_entry(
                1,
                outside,
                AuditEvent::ToolExecuted {
                    tool_id: aivyx_core::ToolId::new(),
                    agent_id: AgentId::new(),
                    action: "file_read".into(),
                    result_summary: "ok".into(),
                },
            ),
        ];

        let summary = compute_summary(&entries, from, to, &zero_cost);
        assert_eq!(summary.tool_executions, 1);
    }

    #[test]
    fn summary_counts_errors() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 1, 6, 0, 0).unwrap();

        let entries = vec![
            make_entry(
                0,
                ts,
                AuditEvent::ToolDenied {
                    tool_id: aivyx_core::ToolId::new(),
                    agent_id: AgentId::new(),
                    action: "shell".into(),
                    reason: "denied".into(),
                },
            ),
            make_entry(
                1,
                ts,
                AuditEvent::HttpAuthFailed {
                    remote_addr: "1.2.3.4".into(),
                    reason: "bad token".into(),
                },
            ),
            make_entry(
                2,
                ts,
                AuditEvent::RateLimitExceeded {
                    remote_addr: "1.2.3.4".into(),
                    tier: "llm".into(),
                    path: "/chat".into(),
                },
            ),
        ];

        let summary = compute_summary(&entries, from, to, &zero_cost);
        assert_eq!(summary.tool_denials, 1);
        assert_eq!(summary.auth_failures, 1);
        assert_eq!(summary.rate_limit_events, 1);
    }

    #[test]
    fn timeline_assigns_to_correct_buckets() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 1, 3, 0, 0).unwrap();
        let hour0 = Utc.with_ymd_and_hms(2026, 3, 1, 0, 30, 0).unwrap();
        let hour2 = Utc.with_ymd_and_hms(2026, 3, 1, 2, 15, 0).unwrap();

        let entries = vec![
            make_entry(
                0,
                hour0,
                AuditEvent::ToolExecuted {
                    tool_id: aivyx_core::ToolId::new(),
                    agent_id: AgentId::new(),
                    action: "file_read".into(),
                    result_summary: "ok".into(),
                },
            ),
            make_entry(
                1,
                hour2,
                AuditEvent::LlmResponseReceived {
                    agent_id: AgentId::new(),
                    provider: "claude".into(),
                    input_tokens: 100,
                    output_tokens: 50,
                    stop_reason: "end_turn".into(),
                },
            ),
        ];

        let timeline = compute_timeline(&entries, from, to, &simple_cost);
        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline[0].tool_executions, 1);
        assert_eq!(timeline[0].llm_requests, 0);
        assert_eq!(timeline[1].tool_executions, 0);
        assert_eq!(timeline[2].llm_requests, 1);
        assert_eq!(timeline[2].input_tokens, 100);
    }

    #[test]
    fn empty_entries_produce_empty_timeline() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 1, 2, 0, 0).unwrap();
        let timeline = compute_timeline(&[], from, to, &zero_cost);
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].llm_requests, 0);
    }

    #[test]
    fn summary_serde_roundtrip() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
        let summary = MetricsSummary {
            from,
            to,
            total_cost_usd: 1.234,
            total_input_tokens: 5000,
            total_output_tokens: 2000,
            llm_requests: 10,
            agent_turns: 5,
            tool_executions: 20,
            tool_denials: 1,
            unique_agents: 2,
            auth_failures: 0,
            rate_limit_events: 0,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let restored: MetricsSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.llm_requests, 10);
        assert!((restored.total_cost_usd - 1.234).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_bucket_serde_roundtrip() {
        let hour = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();
        let bucket = TimelineBucket {
            hour,
            cost_usd: 0.5,
            input_tokens: 1000,
            output_tokens: 500,
            llm_requests: 3,
            agent_turns: 2,
            tool_executions: 5,
            errors: 1,
        };
        let json = serde_json::to_string(&bucket).unwrap();
        let restored: TimelineBucket = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.llm_requests, 3);
        assert_eq!(restored.errors, 1);
    }

    #[test]
    fn unique_agents_counts_distinct() {
        let from = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 1, 6, 0, 0).unwrap();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        let entries = vec![
            make_entry(
                0,
                ts,
                AuditEvent::AgentTurnCompleted {
                    agent_id: agent_a.clone(),
                    session_id: aivyx_core::SessionId::new(),
                    tool_calls_made: 0,
                    tokens_used: 100,
                },
            ),
            make_entry(
                1,
                ts,
                AuditEvent::AgentTurnCompleted {
                    agent_id: agent_a,
                    session_id: aivyx_core::SessionId::new(),
                    tool_calls_made: 0,
                    tokens_used: 200,
                },
            ),
            make_entry(
                2,
                ts,
                AuditEvent::AgentTurnCompleted {
                    agent_id: agent_b,
                    session_id: aivyx_core::SessionId::new(),
                    tool_calls_made: 0,
                    tokens_used: 300,
                },
            ),
        ];

        let summary = compute_summary(&entries, from, to, &zero_cost);
        assert_eq!(summary.agent_turns, 3);
        assert_eq!(summary.unique_agents, 2);
    }
}
