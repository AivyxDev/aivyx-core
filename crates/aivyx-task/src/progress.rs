//! Progress events emitted during task execution.
//!
//! [`ProgressEvent`] is the event type parameterizing
//! [`ProgressSink<ProgressEvent>`](aivyx_core::ProgressSink) from `aivyx-core`.
//! Consumers (CLI, TUI, Engine SSE) subscribe to these for live updates.

use aivyx_core::TaskId;
use serde::{Deserialize, Serialize};

use crate::status::TaskStatus;
use crate::step::StepStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    StatusChanged {
        task_id: TaskId,
        old_status: TaskStatus,
        new_status: TaskStatus,
    },
    StepsPlanned {
        task_id: TaskId,
        step_count: usize,
        step_descriptions: Vec<String>,
    },
    StepStarted {
        task_id: TaskId,
        step_index: usize,
        description: String,
    },
    StepCompleted {
        task_id: TaskId,
        step_index: usize,
        status: StepStatus,
        result_summary: Option<String>,
    },
    ApprovalRequired {
        task_id: TaskId,
        step_index: usize,
        description: String,
        context: String,
    },
    ApprovalResolved {
        task_id: TaskId,
        step_index: usize,
        approved: bool,
    },
    TaskFinished {
        task_id: TaskId,
        status: TaskStatus,
        output: Option<String>,
        error: Option<String>,
    },
    AgentToken {
        task_id: TaskId,
        step_index: usize,
        token: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::{ChannelProgressSink, NoopProgressSink, ProgressSink};

    fn test_task_id() -> TaskId {
        TaskId::new()
    }

    #[test]
    fn serde_roundtrip_status_changed() {
        let event = ProgressEvent::StatusChanged {
            task_id: test_task_id(),
            old_status: TaskStatus::Planning,
            new_status: TaskStatus::Executing,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::StatusChanged { .. }));
    }

    #[test]
    fn serde_roundtrip_steps_planned() {
        let event = ProgressEvent::StepsPlanned {
            task_id: test_task_id(),
            step_count: 3,
            step_descriptions: vec![
                "step 1".into(),
                "step 2".into(),
                "step 3".into(),
            ],
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::StepsPlanned { step_count: 3, .. }));
    }

    #[test]
    fn serde_roundtrip_step_started() {
        let event = ProgressEvent::StepStarted {
            task_id: test_task_id(),
            step_index: 0,
            description: "Fetch data".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::StepStarted { step_index: 0, .. }));
    }

    #[test]
    fn serde_roundtrip_step_completed() {
        let event = ProgressEvent::StepCompleted {
            task_id: test_task_id(),
            step_index: 1,
            status: StepStatus::Completed,
            result_summary: Some("done".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::StepCompleted { step_index: 1, .. }));
    }

    #[test]
    fn serde_roundtrip_approval_required() {
        let event = ProgressEvent::ApprovalRequired {
            task_id: test_task_id(),
            step_index: 2,
            description: "Deploy".into(),
            context: "production deployment".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::ApprovalRequired { step_index: 2, .. }));
    }

    #[test]
    fn serde_roundtrip_approval_resolved() {
        let event = ProgressEvent::ApprovalResolved {
            task_id: test_task_id(),
            step_index: 2,
            approved: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::ApprovalResolved { approved: true, .. }));
    }

    #[test]
    fn serde_roundtrip_task_finished() {
        let event = ProgressEvent::TaskFinished {
            task_id: test_task_id(),
            status: TaskStatus::Completed,
            output: Some("result".into()),
            error: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::TaskFinished { .. }));
    }

    #[test]
    fn serde_roundtrip_agent_token() {
        let event = ProgressEvent::AgentToken {
            task_id: test_task_id(),
            step_index: 0,
            token: "Hello".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProgressEvent::AgentToken { .. }));
    }

    #[tokio::test]
    async fn works_with_noop_progress_sink() {
        let sink = NoopProgressSink::<ProgressEvent>::new();
        let event = ProgressEvent::StatusChanged {
            task_id: test_task_id(),
            old_status: TaskStatus::Planning,
            new_status: TaskStatus::Executing,
        };
        sink.emit(event).await.unwrap();
    }

    #[tokio::test]
    async fn works_with_channel_progress_sink() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let sink = ChannelProgressSink::new(tx);
        let task_id = test_task_id();
        let event = ProgressEvent::StepStarted {
            task_id,
            step_index: 0,
            description: "test step".into(),
        };
        sink.emit(event).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert!(matches!(received, ProgressEvent::StepStarted { step_index: 0, .. }));
    }
}
