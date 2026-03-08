//! Task lifecycle status.
//!
//! Defined in `aivyx-core` so that both `aivyx-task` and `aivyx-audit` can
//! reference it without creating a circular dependency.

use crate::a2a::A2aTaskState;
use serde::{Deserialize, Serialize};

/// Internal task lifecycle status.
///
/// Maps to A2A states per the mapping in [`A2aTaskState`]:
/// - `Planning` / `Planned` → `Submitted`
/// - `Executing` / `Verifying` → `Working`
/// - `AwaitingApproval` → `InputRequired`
/// - `Completed` → `Completed`
/// - `Failed` → `Failed`
/// - `Cancelled` → `Canceled`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Planning,
    Planned,
    Executing,
    Verifying,
    AwaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    /// Convert to the corresponding A2A task state.
    pub fn to_a2a_state(&self) -> A2aTaskState {
        match self {
            TaskStatus::Planning | TaskStatus::Planned => A2aTaskState::Submitted,
            TaskStatus::Executing | TaskStatus::Verifying => A2aTaskState::Working,
            TaskStatus::AwaitingApproval => A2aTaskState::InputRequired,
            TaskStatus::Completed => A2aTaskState::Completed,
            TaskStatus::Failed => A2aTaskState::Failed,
            TaskStatus::Cancelled => A2aTaskState::Canceled,
        }
    }

    /// Returns `true` if the task has reached a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }

    /// Returns `true` if the task is actively progressing.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            TaskStatus::Planning
                | TaskStatus::Planned
                | TaskStatus::Executing
                | TaskStatus::Verifying
        )
    }
}

impl From<TaskStatus> for A2aTaskState {
    fn from(status: TaskStatus) -> Self {
        status.to_a2a_state()
    }
}

/// Maps an A2A task state to the most natural internal `TaskStatus`.
///
/// The mapping is lossy because multiple internal states map to a single A2A
/// state (e.g., both `Planning` and `Planned` → `Submitted`). This impl
/// picks the most actionable internal state for each A2A state:
/// - `Submitted` → `Planning` (the task needs decomposition)
/// - `Working` → `Executing` (the task is actively running)
/// - `InputRequired` → `AwaitingApproval`
/// - `Completed` → `Completed`
/// - `Failed` → `Failed`
/// - `Canceled` → `Cancelled`
impl From<A2aTaskState> for TaskStatus {
    fn from(state: A2aTaskState) -> Self {
        match state {
            A2aTaskState::Submitted => TaskStatus::Planning,
            A2aTaskState::Working => TaskStatus::Executing,
            A2aTaskState::InputRequired => TaskStatus::AwaitingApproval,
            A2aTaskState::Completed => TaskStatus::Completed,
            A2aTaskState::Failed => TaskStatus::Failed,
            A2aTaskState::Canceled => TaskStatus::Cancelled,
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Planning => write!(f, "planning"),
            TaskStatus::Planned => write!(f, "planned"),
            TaskStatus::Executing => write!(f, "executing"),
            TaskStatus::Verifying => write!(f, "verifying"),
            TaskStatus::AwaitingApproval => write!(f, "awaiting_approval"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
            TaskStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_a2a_state_mapping() {
        assert_eq!(TaskStatus::Planning.to_a2a_state(), A2aTaskState::Submitted);
        assert_eq!(TaskStatus::Planned.to_a2a_state(), A2aTaskState::Submitted);
        assert_eq!(TaskStatus::Executing.to_a2a_state(), A2aTaskState::Working);
        assert_eq!(TaskStatus::Verifying.to_a2a_state(), A2aTaskState::Working);
        assert_eq!(
            TaskStatus::AwaitingApproval.to_a2a_state(),
            A2aTaskState::InputRequired
        );
        assert_eq!(
            TaskStatus::Completed.to_a2a_state(),
            A2aTaskState::Completed
        );
        assert_eq!(TaskStatus::Failed.to_a2a_state(), A2aTaskState::Failed);
        assert_eq!(TaskStatus::Cancelled.to_a2a_state(), A2aTaskState::Canceled);
    }

    #[test]
    fn is_terminal() {
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
        assert!(!TaskStatus::Planning.is_terminal());
        assert!(!TaskStatus::Executing.is_terminal());
        assert!(!TaskStatus::AwaitingApproval.is_terminal());
    }

    #[test]
    fn is_active() {
        assert!(TaskStatus::Planning.is_active());
        assert!(TaskStatus::Planned.is_active());
        assert!(TaskStatus::Executing.is_active());
        assert!(TaskStatus::Verifying.is_active());
        assert!(!TaskStatus::AwaitingApproval.is_active());
        assert!(!TaskStatus::Completed.is_active());
    }

    #[test]
    fn from_a2a_roundtrip_terminal_states() {
        for (internal, a2a) in [
            (TaskStatus::Completed, A2aTaskState::Completed),
            (TaskStatus::Failed, A2aTaskState::Failed),
            (TaskStatus::Cancelled, A2aTaskState::Canceled),
            (TaskStatus::AwaitingApproval, A2aTaskState::InputRequired),
        ] {
            let converted: A2aTaskState = internal.into();
            assert_eq!(converted, a2a);
            let back: TaskStatus = a2a.into();
            assert_eq!(back, internal);
        }
    }

    #[test]
    fn serde_roundtrip() {
        for status in [
            TaskStatus::Planning,
            TaskStatus::Planned,
            TaskStatus::Executing,
            TaskStatus::Verifying,
            TaskStatus::AwaitingApproval,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn display_formatting() {
        assert_eq!(TaskStatus::Planning.to_string(), "planning");
        assert_eq!(
            TaskStatus::AwaitingApproval.to_string(),
            "awaiting_approval"
        );
        assert_eq!(TaskStatus::Cancelled.to_string(), "cancelled");
    }
}
