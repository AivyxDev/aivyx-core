use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Configuration for the tool abuse detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbuseDetectorConfig {
    /// Sliding window duration in seconds.
    pub window_secs: u64,
    /// Maximum tool calls per window before alerting.
    pub max_calls_per_window: usize,
    /// Maximum denied calls per window before alerting.
    pub max_denials_per_window: usize,
    /// Maximum unique tools per window before scope escalation alert.
    pub max_unique_tools_per_window: usize,
    /// Whether detection is enabled.
    pub enabled: bool,
}

impl Default for AbuseDetectorConfig {
    fn default() -> Self {
        Self {
            window_secs: 60,
            max_calls_per_window: 50,
            max_denials_per_window: 5,
            max_unique_tools_per_window: 10,
            enabled: true,
        }
    }
}

/// A tool call event in the sliding window.
struct ToolEvent {
    timestamp: DateTime<Utc>,
    tool_name: String,
    denied: bool,
}

/// An alert triggered by anomalous tool usage patterns.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum AbuseAlert {
    /// Agent is making tool calls at an unusually high rate.
    HighFrequency {
        agent: String,
        calls: usize,
        window_secs: u64,
    },
    /// Agent is repeatedly being denied tool access.
    RepeatedDenials {
        agent: String,
        denials: usize,
        window_secs: u64,
    },
    /// Agent is trying many different tools in quick succession.
    ScopeEscalation {
        agent: String,
        unique_tools: usize,
        window_secs: u64,
    },
}

/// Sliding-window tool abuse detector.
///
/// Maintains per-agent event windows and checks configurable thresholds
/// after each tool call. Thread-safe via internal `Mutex`.
pub struct AbuseDetector {
    config: AbuseDetectorConfig,
    windows: Mutex<HashMap<String, VecDeque<ToolEvent>>>,
}

impl AbuseDetector {
    pub fn new(config: AbuseDetectorConfig) -> Self {
        Self {
            config,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Record a tool call and return any triggered alerts.
    ///
    /// Returns an empty vec if no thresholds are exceeded.
    pub fn record_tool_call(
        &self,
        agent_id: &str,
        tool_name: &str,
        denied: bool,
    ) -> Vec<AbuseAlert> {
        if !self.config.enabled {
            return Vec::new();
        }

        let now = Utc::now();
        let event = ToolEvent {
            timestamp: now,
            tool_name: tool_name.to_string(),
            denied,
        };

        let mut windows = self.windows.lock().unwrap();
        let events = windows.entry(agent_id.to_string()).or_default();
        events.push_back(event);

        // Prune old events
        let cutoff = now - chrono::Duration::seconds(self.config.window_secs as i64);
        while events.front().is_some_and(|e| e.timestamp < cutoff) {
            events.pop_front();
        }

        // Check thresholds
        let mut alerts = Vec::new();

        // 1. High frequency
        if events.len() >= self.config.max_calls_per_window {
            alerts.push(AbuseAlert::HighFrequency {
                agent: agent_id.to_string(),
                calls: events.len(),
                window_secs: self.config.window_secs,
            });
        }

        // 2. Repeated denials
        let denial_count = events.iter().filter(|e| e.denied).count();
        if denial_count >= self.config.max_denials_per_window {
            alerts.push(AbuseAlert::RepeatedDenials {
                agent: agent_id.to_string(),
                denials: denial_count,
                window_secs: self.config.window_secs,
            });
        }

        // 3. Scope escalation (many unique tools)
        let unique_tools: HashSet<&str> = events.iter().map(|e| e.tool_name.as_str()).collect();
        if unique_tools.len() >= self.config.max_unique_tools_per_window {
            alerts.push(AbuseAlert::ScopeEscalation {
                agent: agent_id.to_string(),
                unique_tools: unique_tools.len(),
                window_secs: self.config.window_secs,
            });
        }

        alerts
    }

    /// Get the current configuration.
    pub fn config(&self) -> &AbuseDetectorConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = AbuseDetectorConfig::default();
        assert_eq!(config.window_secs, 60);
        assert_eq!(config.max_calls_per_window, 50);
        assert_eq!(config.max_denials_per_window, 5);
        assert_eq!(config.max_unique_tools_per_window, 10);
        assert!(config.enabled);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = AbuseDetectorConfig {
            window_secs: 120,
            max_calls_per_window: 100,
            max_denials_per_window: 10,
            max_unique_tools_per_window: 20,
            enabled: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AbuseDetectorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.window_secs, 120);
        assert!(!parsed.enabled);
    }

    #[test]
    fn no_alert_below_threshold() {
        let detector = AbuseDetector::new(AbuseDetectorConfig {
            max_calls_per_window: 5,
            ..Default::default()
        });
        for _ in 0..4 {
            let alerts = detector.record_tool_call("agent-1", "shell", false);
            assert!(alerts.is_empty());
        }
    }

    #[test]
    fn high_frequency_alert() {
        let detector = AbuseDetector::new(AbuseDetectorConfig {
            max_calls_per_window: 3,
            ..Default::default()
        });
        detector.record_tool_call("agent-1", "shell", false);
        detector.record_tool_call("agent-1", "shell", false);
        let alerts = detector.record_tool_call("agent-1", "shell", false);
        assert!(
            alerts
                .iter()
                .any(|a| matches!(a, AbuseAlert::HighFrequency { calls: 3, .. }))
        );
    }

    #[test]
    fn repeated_denial_alert() {
        let detector = AbuseDetector::new(AbuseDetectorConfig {
            max_denials_per_window: 2,
            max_calls_per_window: 100, // don't trigger high freq
            ..Default::default()
        });
        detector.record_tool_call("agent-1", "shell", true);
        let alerts = detector.record_tool_call("agent-1", "web_search", true);
        assert!(
            alerts
                .iter()
                .any(|a| matches!(a, AbuseAlert::RepeatedDenials { denials: 2, .. }))
        );
    }

    #[test]
    fn scope_escalation_alert() {
        let detector = AbuseDetector::new(AbuseDetectorConfig {
            max_unique_tools_per_window: 3,
            max_calls_per_window: 100, // don't trigger high freq
            ..Default::default()
        });
        detector.record_tool_call("agent-1", "shell", false);
        detector.record_tool_call("agent-1", "web_search", false);
        let alerts = detector.record_tool_call("agent-1", "file_read", false);
        assert!(alerts.iter().any(|a| matches!(
            a,
            AbuseAlert::ScopeEscalation {
                unique_tools: 3,
                ..
            }
        )));
    }

    #[test]
    fn multiple_agents_independent() {
        let detector = AbuseDetector::new(AbuseDetectorConfig {
            max_calls_per_window: 3,
            ..Default::default()
        });
        detector.record_tool_call("agent-1", "shell", false);
        detector.record_tool_call("agent-1", "shell", false);
        detector.record_tool_call("agent-2", "shell", false);
        // agent-1 at 2, agent-2 at 1 — neither should trigger
        let alerts1 = detector.record_tool_call("agent-2", "shell", false);
        assert!(alerts1.is_empty()); // agent-2 only at 2
        let alerts2 = detector.record_tool_call("agent-1", "shell", false);
        assert!(!alerts2.is_empty()); // agent-1 at 3 = triggered
    }

    #[test]
    fn disabled_detector_returns_no_alerts() {
        let detector = AbuseDetector::new(AbuseDetectorConfig {
            enabled: false,
            max_calls_per_window: 1,
            ..Default::default()
        });
        let alerts = detector.record_tool_call("agent-1", "shell", false);
        assert!(alerts.is_empty());
    }

    #[test]
    fn alert_serde_roundtrip() {
        let alert = AbuseAlert::HighFrequency {
            agent: "test-agent".into(),
            calls: 55,
            window_secs: 60,
        };
        let json = serde_json::to_string(&alert).unwrap();
        assert!(json.contains("\"type\":\"HighFrequency\""));
        assert!(json.contains("\"calls\":55"));
    }
}
