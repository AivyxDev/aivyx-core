//! Async audit sink trait for pluggable event consumers.

use async_trait::async_trait;

use crate::event::AuditEvent;
use aivyx_core::Result;

/// An asynchronous consumer of audit events.
#[async_trait]
pub trait AuditSink: Send + Sync {
    /// Process a single audit event. Implementations may write to remote
    /// services, databases, or other sinks.
    async fn consume(&self, event: &AuditEvent) -> Result<()>;
}
