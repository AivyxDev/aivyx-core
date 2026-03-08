//! Task step — a single decomposed sub-goal within a task.

use serde::{Deserialize, Serialize};

/// Status of an individual task step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl std::fmt::Display for StepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepStatus::Pending => write!(f, "pending"),
            StepStatus::Running => write!(f, "running"),
            StepStatus::Completed => write!(f, "completed"),
            StepStatus::Failed => write!(f, "failed"),
            StepStatus::Skipped => write!(f, "skipped"),
        }
    }
}

/// A single decomposed sub-goal within a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    /// Zero-based index of this step within the task plan.
    pub index: usize,
    /// Human-readable description of what this step accomplishes.
    pub description: String,
    /// Current status of this step.
    pub status: StepStatus,
    /// Whether this step requires human approval before execution.
    pub requires_approval: bool,
    /// Output or result produced by this step, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Error message if this step failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Names of tools invoked during this step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_used: Vec<String>,
    /// Wall-clock duration of this step in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl TaskStep {
    /// Create a new step in `Pending` status with sensible defaults.
    pub fn new(index: usize, description: impl Into<String>) -> Self {
        Self {
            index,
            description: description.into(),
            status: StepStatus::Pending,
            requires_approval: false,
            result: None,
            error: None,
            tools_used: Vec::new(),
            duration_ms: None,
        }
    }

    /// Mark this step as requiring human approval before execution.
    pub fn with_approval(mut self) -> Self {
        self.requires_approval = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_pending_step() {
        let step = TaskStep::new(0, "Fetch data from API");
        assert_eq!(step.index, 0);
        assert_eq!(step.description, "Fetch data from API");
        assert_eq!(step.status, StepStatus::Pending);
        assert!(!step.requires_approval);
        assert!(step.result.is_none());
        assert!(step.error.is_none());
        assert!(step.tools_used.is_empty());
        assert!(step.duration_ms.is_none());
    }

    #[test]
    fn with_approval_sets_flag() {
        let step = TaskStep::new(1, "Deploy to production").with_approval();
        assert!(step.requires_approval);
    }

    #[test]
    fn step_status_display() {
        assert_eq!(StepStatus::Pending.to_string(), "pending");
        assert_eq!(StepStatus::Running.to_string(), "running");
        assert_eq!(StepStatus::Completed.to_string(), "completed");
        assert_eq!(StepStatus::Failed.to_string(), "failed");
        assert_eq!(StepStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn step_status_serde_roundtrip() {
        for status in [
            StepStatus::Pending,
            StepStatus::Running,
            StepStatus::Completed,
            StepStatus::Failed,
            StepStatus::Skipped,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: StepStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn task_step_serde_roundtrip_all_fields() {
        let step = TaskStep {
            index: 2,
            description: "Run integration tests".into(),
            status: StepStatus::Completed,
            requires_approval: true,
            result: Some("All 42 tests passed".into()),
            error: None,
            tools_used: vec!["shell_exec".into(), "file_read".into()],
            duration_ms: Some(3500),
        };

        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["index"], 2);
        assert_eq!(json["description"], "Run integration tests");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["requires_approval"], true);
        assert_eq!(json["result"], "All 42 tests passed");
        assert!(json.get("error").is_none());
        assert_eq!(json["tools_used"][0], "shell_exec");
        assert_eq!(json["tools_used"][1], "file_read");
        assert_eq!(json["duration_ms"], 3500);

        let restored: TaskStep = serde_json::from_value(json).unwrap();
        assert_eq!(restored.index, 2);
        assert_eq!(restored.status, StepStatus::Completed);
        assert!(restored.requires_approval);
        assert_eq!(restored.result.as_deref(), Some("All 42 tests passed"));
        assert!(restored.error.is_none());
        assert_eq!(restored.tools_used.len(), 2);
        assert_eq!(restored.duration_ms, Some(3500));
    }

    #[test]
    fn task_step_serde_minimal() {
        let step = TaskStep::new(0, "Simple step");
        let json = serde_json::to_value(&step).unwrap();

        // Optional/empty fields should be omitted
        assert!(json.get("result").is_none());
        assert!(json.get("error").is_none());
        assert!(json.get("tools_used").is_none());
        assert!(json.get("duration_ms").is_none());

        let restored: TaskStep = serde_json::from_value(json).unwrap();
        assert_eq!(restored.index, 0);
        assert_eq!(restored.status, StepStatus::Pending);
    }
}
