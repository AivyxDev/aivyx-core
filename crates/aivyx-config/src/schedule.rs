//! Scheduler configuration.
//!
//! Each [`ScheduleEntry`] defines a named background task: a cron expression,
//! the agent to run it, and the prompt to send. The engine fires these on a
//! timer and optionally stores results as notifications.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single scheduled background task.
///
/// Stored as a `[[schedules]]` entry in `config.toml`. Each schedule fires
/// an agent turn on a cron-driven timer and optionally stores the result as
/// a notification surfaced on the next interactive session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleEntry {
    /// Unique name for this schedule (slug-style, e.g., `"morning-digest"`).
    pub name: String,
    /// Standard 5-field cron expression (minute hour dom month dow).
    ///
    /// Example: `"0 7 * * *"` = 7:00 AM every day.
    pub cron: String,
    /// Agent profile name to run for this task.
    pub agent: String,
    /// Prompt to send to the agent when the schedule fires.
    pub prompt: String,
    /// Whether the result should be stored as a notification surfaced on
    /// the next interactive turn.
    #[serde(default = "default_true")]
    pub notify: bool,
    /// Whether this schedule is currently active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// When this entry was created.
    pub created_at: DateTime<Utc>,
    /// When this entry last fired (`None` if never run).
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
}

fn default_true() -> bool {
    true
}

impl ScheduleEntry {
    /// Create a new schedule entry with defaults.
    pub fn new(
        name: impl Into<String>,
        cron: impl Into<String>,
        agent: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            cron: cron.into(),
            agent: agent.into(),
            prompt: prompt.into(),
            notify: true,
            enabled: true,
            created_at: Utc::now(),
            last_run_at: None,
        }
    }

    /// Built-in morning digest schedule entry.
    pub fn daily_digest(agent: &str) -> Self {
        Self::new(
            "morning-digest",
            "0 7 * * *",
            agent,
            "Generate a concise morning digest. Summarize: recent project activity from memory, \
             any pending tasks, and anything else I should know about today. Be brief.",
        )
    }
}

/// Validate a cron expression string.
///
/// Returns `Ok(())` if the expression is a valid 5-field cron expression,
/// or `Err(AivyxError::Scheduler(...))` with a descriptive message.
pub fn validate_cron(expr: &str) -> aivyx_core::Result<()> {
    croner::Cron::new(expr).parse().map(|_| ()).map_err(|e| {
        aivyx_core::AivyxError::Scheduler(format!("invalid cron expression '{expr}': {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_entry_new() {
        let entry = ScheduleEntry::new("test", "0 7 * * *", "assistant", "Hello");
        assert_eq!(entry.name, "test");
        assert_eq!(entry.cron, "0 7 * * *");
        assert_eq!(entry.agent, "assistant");
        assert_eq!(entry.prompt, "Hello");
        assert!(entry.notify);
        assert!(entry.enabled);
        assert!(entry.last_run_at.is_none());
    }

    #[test]
    fn schedule_entry_serde_roundtrip() {
        let entry = ScheduleEntry::new("morning-check", "30 8 * * 1-5", "assistant", "Status?");
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ScheduleEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "morning-check");
        assert_eq!(parsed.cron, "30 8 * * 1-5");
        assert_eq!(parsed.agent, "assistant");
        assert!(parsed.notify);
        assert!(parsed.enabled);
    }

    #[test]
    fn daily_digest_entry() {
        let entry = ScheduleEntry::daily_digest("researcher");
        assert_eq!(entry.name, "morning-digest");
        assert_eq!(entry.cron, "0 7 * * *");
        assert_eq!(entry.agent, "researcher");
        assert!(entry.prompt.contains("digest"));
    }

    #[test]
    fn validate_cron_accepts_valid() {
        assert!(validate_cron("0 7 * * *").is_ok());
        assert!(validate_cron("*/15 * * * *").is_ok());
        assert!(validate_cron("30 8 * * 1-5").is_ok());
        assert!(validate_cron("0 0 1 * *").is_ok());
    }

    #[test]
    fn validate_cron_rejects_invalid() {
        assert!(validate_cron("not-a-cron").is_err());
        assert!(validate_cron("").is_err());
    }
}
