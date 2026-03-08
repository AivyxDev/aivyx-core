//! Task orchestration contract.
//!
//! [`TaskRunner`] defines how a task is driven through its lifecycle.
//! Concrete implementations live at call sites (engine, CLI) where
//! [`Agent`](aivyx_agent::Agent) is available.

use std::sync::Arc;

use async_trait::async_trait;

use aivyx_audit::AuditLog;
use aivyx_core::{ProgressSink, Result, TaskId};
use aivyx_crypto::MasterKey;

use crate::progress::ProgressEvent;
use crate::store::TaskStore;
use crate::task::Task;

/// How approval gates are handled during task execution.
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request_approval(
        &self,
        task_id: &TaskId,
        step_index: usize,
        description: &str,
        context: &str,
    ) -> Result<bool>;
}

/// Auto-approve all approval gates.
pub struct AutoApproveHandler;

#[async_trait]
impl ApprovalHandler for AutoApproveHandler {
    async fn request_approval(
        &self,
        _task_id: &TaskId,
        _step_index: usize,
        _description: &str,
        _context: &str,
    ) -> Result<bool> {
        Ok(true)
    }
}

/// Context shared across a task execution run.
pub struct TaskContext {
    pub store: Arc<TaskStore>,
    pub key: MasterKey,
    pub audit_log: Option<AuditLog>,
    pub progress: Arc<dyn ProgressSink<ProgressEvent>>,
    pub approval: Arc<dyn ApprovalHandler>,
    pub cancel: tokio_util::sync::CancellationToken,
}

/// Trait for task orchestration.
///
/// Implementors drive an agent through a multi-step task. The trait is defined
/// here so that `aivyx-task` stays independent of `aivyx-agent`.
#[async_trait]
pub trait TaskRunner: Send + Sync {
    /// Execute a task from its current state to completion (or failure/cancellation).
    async fn run(&self, task: &mut Task, ctx: &TaskContext) -> Result<()>;
    /// Resume a previously paused or failed task from its current step.
    async fn resume(&self, task: &mut Task, ctx: &TaskContext) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn auto_approve_handler_returns_ok_true() {
        let handler = AutoApproveHandler;
        let task_id = TaskId::new();
        let result = handler
            .request_approval(&task_id, 0, "deploy", "production")
            .await
            .unwrap();
        assert!(result);
    }

    #[test]
    fn task_runner_is_object_safe() {
        let _: Option<Box<dyn TaskRunner>> = None;
    }

    #[test]
    fn approval_handler_is_object_safe() {
        let _: Option<Box<dyn ApprovalHandler>> = None;
    }
}
