//! Task lifecycle status — re-exported from `aivyx-core`.
//!
//! The canonical [`TaskStatus`] enum lives in `aivyx-core::task_status` so that
//! both `aivyx-task` and `aivyx-audit` can use it without circular dependencies.
//! This module re-exports it for backwards compatibility.

pub use aivyx_core::TaskStatus;

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::a2a::A2aTaskState;

    #[test]
    fn to_a2a_state_planning() {
        assert_eq!(TaskStatus::Planning.to_a2a_state(), A2aTaskState::Submitted);
    }

    #[test]
    fn to_a2a_state_planned() {
        assert_eq!(TaskStatus::Planned.to_a2a_state(), A2aTaskState::Submitted);
    }

    #[test]
    fn to_a2a_state_executing() {
        assert_eq!(TaskStatus::Executing.to_a2a_state(), A2aTaskState::Working);
    }

    #[test]
    fn to_a2a_state_verifying() {
        assert_eq!(TaskStatus::Verifying.to_a2a_state(), A2aTaskState::Working);
    }

    #[test]
    fn to_a2a_state_awaiting_approval() {
        assert_eq!(
            TaskStatus::AwaitingApproval.to_a2a_state(),
            A2aTaskState::InputRequired
        );
    }

    #[test]
    fn to_a2a_state_completed() {
        assert_eq!(
            TaskStatus::Completed.to_a2a_state(),
            A2aTaskState::Completed
        );
    }

    #[test]
    fn to_a2a_state_failed() {
        assert_eq!(TaskStatus::Failed.to_a2a_state(), A2aTaskState::Failed);
    }

    #[test]
    fn to_a2a_state_cancelled() {
        assert_eq!(TaskStatus::Cancelled.to_a2a_state(), A2aTaskState::Canceled);
    }

    #[test]
    fn is_terminal_true() {
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn is_terminal_false() {
        assert!(!TaskStatus::Planning.is_terminal());
        assert!(!TaskStatus::Planned.is_terminal());
        assert!(!TaskStatus::Executing.is_terminal());
        assert!(!TaskStatus::Verifying.is_terminal());
        assert!(!TaskStatus::AwaitingApproval.is_terminal());
    }

    #[test]
    fn is_active_true() {
        assert!(TaskStatus::Planning.is_active());
        assert!(TaskStatus::Planned.is_active());
        assert!(TaskStatus::Executing.is_active());
        assert!(TaskStatus::Verifying.is_active());
    }

    #[test]
    fn is_active_false() {
        assert!(!TaskStatus::AwaitingApproval.is_active());
        assert!(!TaskStatus::Completed.is_active());
        assert!(!TaskStatus::Failed.is_active());
        assert!(!TaskStatus::Cancelled.is_active());
    }

    #[test]
    fn from_task_status_for_a2a() {
        let state: A2aTaskState = TaskStatus::Executing.into();
        assert_eq!(state, A2aTaskState::Working);
    }

    #[test]
    fn from_a2a_submitted_to_planning() {
        let status: TaskStatus = A2aTaskState::Submitted.into();
        assert_eq!(status, TaskStatus::Planning);
    }

    #[test]
    fn from_a2a_working_to_executing() {
        let status: TaskStatus = A2aTaskState::Working.into();
        assert_eq!(status, TaskStatus::Executing);
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
        assert_eq!(TaskStatus::Planned.to_string(), "planned");
        assert_eq!(TaskStatus::Executing.to_string(), "executing");
        assert_eq!(TaskStatus::Verifying.to_string(), "verifying");
        assert_eq!(
            TaskStatus::AwaitingApproval.to_string(),
            "awaiting_approval"
        );
        assert_eq!(TaskStatus::Completed.to_string(), "completed");
        assert_eq!(TaskStatus::Failed.to_string(), "failed");
        assert_eq!(TaskStatus::Cancelled.to_string(), "cancelled");
    }
}
