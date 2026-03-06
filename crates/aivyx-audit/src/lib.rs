//! Tamper-evident audit logging for the aivyx framework.
//!
//! Every security-relevant action is recorded as an [`AuditEvent`] and
//! appended to an HMAC-SHA256 chained log. The chain ensures that any
//! modification to past entries is detectable via [`AuditLog::verify`].

pub mod event;
pub mod export;
pub mod log;
pub mod metrics;
pub mod retention;
pub mod search;
pub mod sink;

pub use event::AuditEvent;
pub use export::{export_csv, export_json};
pub use log::{AuditEntry, AuditLog, VerifyResult};
pub use metrics::{MetricsSummary, TimelineBucket, compute_summary, compute_timeline};
pub use retention::{PruneResult, prune};
pub use search::AuditFilter;
pub use sink::AuditSink;
