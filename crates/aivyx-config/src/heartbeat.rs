//! Heartbeat configuration.
//!
//! The heartbeat is a periodic self-check loop where an agent reviews its
//! current state and autonomously decides whether to take action. Unlike the
//! scheduler (which fires static prompts on cron), the heartbeat gathers a
//! context diff and lets the agent reason about what — if anything — needs
//! attention.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Heartbeat configuration block.
///
/// Stored as `[heartbeat]` in `config.toml`. When enabled, the engine spawns
/// a background loop that periodically gathers context and runs an agent turn
/// to decide if proactive action is needed.
///
/// The heartbeat is context-aware: it skips the LLM call entirely when nothing
/// has changed since the last beat, saving tokens and cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// Whether the heartbeat loop is active.
    #[serde(default)]
    pub enabled: bool,

    /// Minutes between heartbeat ticks. The agent only runs if there is
    /// context to review; quiet ticks are free (no LLM call).
    ///
    /// Default: 30 minutes.
    #[serde(default = "default_interval")]
    pub interval_minutes: u64,

    /// Agent profile name to use for heartbeat reasoning.
    ///
    /// Default: `"assistant"`.
    #[serde(default = "default_agent")]
    pub agent: String,

    // ── Context sources ─────────────────────────────────────────────
    /// Include pending notifications from background schedules.
    #[serde(default = "default_true")]
    pub check_notifications: bool,

    /// Include recent schedule results since last heartbeat.
    #[serde(default = "default_true")]
    pub check_schedules: bool,

    /// Include memory health (consolidation candidates, stale items).
    #[serde(default = "default_true")]
    pub check_memory: bool,

    // ── User-defined goals ──────────────────────────────────────────
    /// Natural-language objectives the agent should consider each beat.
    ///
    /// Examples:
    /// - "Summarize any new research findings"
    /// - "Prepare a morning briefing between 7-9am"
    /// - "Check if any monitored services have changed status"
    #[serde(default)]
    pub goals: Vec<String>,

    // ── Autonomy constraints ────────────────────────────────────────
    /// Whether the heartbeat agent may proactively send channel messages
    /// (e.g., Telegram). When `false`, findings are stored as notifications
    /// only.
    #[serde(default)]
    pub can_send_channel_messages: bool,

    /// Whether the heartbeat agent may store findings as notifications
    /// surfaced on the next interactive turn.
    #[serde(default = "default_true")]
    pub can_store_notifications: bool,

    /// Whether the heartbeat agent may trigger memory consolidation
    /// (merge similar memories, prune stale entries).
    #[serde(default)]
    pub can_consolidate_memory: bool,

    // ── Outcome & consolidation ───────────────────────────────────
    /// Include recent outcome statistics (tool success rates, failure counts)
    /// in the heartbeat context for the agent to reason about.
    #[serde(default = "default_true")]
    pub check_outcomes: bool,

    /// Automatically trigger memory consolidation when the memory store
    /// exceeds this many entries. Set to `0` to disable threshold-based
    /// consolidation (the agent can still request it explicitly).
    #[serde(default = "default_consolidation_threshold")]
    pub consolidation_threshold: usize,

    // ── Mission monitoring ────────────────────────────────────────
    /// Monitor active missions and surface stalled or failed ones in
    /// the heartbeat context. The agent can then resume or escalate them.
    #[serde(default)]
    pub check_missions: bool,

    /// Minutes after which an executing mission is considered stalled.
    /// Only relevant when `check_missions` is `true`.
    #[serde(default = "default_mission_stall_minutes")]
    pub mission_stall_minutes: u64,

    // ── Runtime state ───────────────────────────────────────────────
    /// Timestamp of the last heartbeat tick (set by the runtime).
    #[serde(default)]
    pub last_beat_at: Option<DateTime<Utc>>,
}

fn default_interval() -> u64 {
    30
}

fn default_agent() -> String {
    "assistant".into()
}

fn default_true() -> bool {
    true
}

fn default_consolidation_threshold() -> usize {
    200
}

fn default_mission_stall_minutes() -> u64 {
    60
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: default_interval(),
            agent: default_agent(),
            check_notifications: true,
            check_schedules: true,
            check_memory: true,
            goals: Vec::new(),
            can_send_channel_messages: false,
            can_store_notifications: true,
            can_consolidate_memory: false,
            check_outcomes: true,
            consolidation_threshold: default_consolidation_threshold(),
            check_missions: false,
            mission_stall_minutes: default_mission_stall_minutes(),
            last_beat_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let hb = HeartbeatConfig::default();
        assert!(!hb.enabled);
        assert_eq!(hb.interval_minutes, 30);
        assert_eq!(hb.agent, "assistant");
        assert!(hb.check_notifications);
        assert!(hb.check_schedules);
        assert!(hb.check_memory);
        assert!(hb.goals.is_empty());
        assert!(!hb.can_send_channel_messages);
        assert!(hb.can_store_notifications);
        assert!(!hb.can_consolidate_memory);
        assert!(hb.check_outcomes);
        assert_eq!(hb.consolidation_threshold, 200);
        assert!(!hb.check_missions);
        assert_eq!(hb.mission_stall_minutes, 60);
        assert!(hb.last_beat_at.is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let mut hb = HeartbeatConfig::default();
        hb.enabled = true;
        hb.interval_minutes = 15;
        hb.agent = "monitor".into();
        hb.goals = vec!["Check CI pipelines".into(), "Summarize new research".into()];
        hb.can_consolidate_memory = true;

        let toml_str = toml::to_string(&hb).unwrap();
        let parsed: HeartbeatConfig = toml::from_str(&toml_str).unwrap();

        assert!(parsed.enabled);
        assert_eq!(parsed.interval_minutes, 15);
        assert_eq!(parsed.agent, "monitor");
        assert_eq!(parsed.goals.len(), 2);
        assert!(parsed.can_consolidate_memory);
    }

    #[test]
    fn deserialize_minimal() {
        // Only `enabled` set — everything else should use defaults
        let toml_str = r#"enabled = true"#;
        let parsed: HeartbeatConfig = toml::from_str(toml_str).unwrap();

        assert!(parsed.enabled);
        assert_eq!(parsed.interval_minutes, 30);
        assert_eq!(parsed.agent, "assistant");
        assert!(parsed.check_notifications);
    }

    #[test]
    fn deserialize_with_goals() {
        let toml_str = r#"
enabled = true
interval_minutes = 10
agent = "watcher"
goals = [
    "Prepare morning briefing between 7-9am",
    "Alert if any monitored service is down",
]
can_send_channel_messages = true
"#;
        let parsed: HeartbeatConfig = toml::from_str(toml_str).unwrap();

        assert!(parsed.enabled);
        assert_eq!(parsed.interval_minutes, 10);
        assert_eq!(parsed.agent, "watcher");
        assert_eq!(parsed.goals.len(), 2);
        assert!(parsed.goals[0].contains("morning briefing"));
        assert!(parsed.can_send_channel_messages);
    }

    #[test]
    fn json_roundtrip() {
        let hb = HeartbeatConfig::default();
        let json = serde_json::to_string(&hb).unwrap();
        let parsed: HeartbeatConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.interval_minutes, hb.interval_minutes);
        assert_eq!(parsed.agent, hb.agent);
    }

    #[test]
    fn deserialize_with_missions_and_outcomes() {
        let toml_str = r#"
enabled = true
check_outcomes = true
check_missions = true
mission_stall_minutes = 45
consolidation_threshold = 100
can_consolidate_memory = true
"#;
        let parsed: HeartbeatConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.check_outcomes);
        assert!(parsed.check_missions);
        assert_eq!(parsed.mission_stall_minutes, 45);
        assert_eq!(parsed.consolidation_threshold, 100);
        assert!(parsed.can_consolidate_memory);
    }

    #[test]
    fn backward_compat_no_new_fields() {
        // Existing config without new fields should still parse with defaults.
        let toml_str = r#"
enabled = true
interval_minutes = 15
"#;
        let parsed: HeartbeatConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.check_outcomes); // default true
        assert!(!parsed.check_missions); // default false
        assert_eq!(parsed.consolidation_threshold, 200); // default
        assert_eq!(parsed.mission_stall_minutes, 60); // default
    }
}
