//! Task lifecycle system for the Aivyx ecosystem.
//!
//! Provides the persistent [`Task`] struct that ties together planning,
//! execution, approval gates, and progress reporting. Maps to A2A task
//! states for interoperability.

pub mod progress;
pub mod runner;
pub mod status;
pub mod step;
pub mod store;
pub mod task;

pub use progress::ProgressEvent;
pub use runner::{ApprovalHandler, AutoApproveHandler, TaskContext, TaskRunner};
pub use status::TaskStatus;
pub use step::{StepStatus, TaskStep};
pub use store::{TaskFilter, TaskStore};
pub use task::{Task, TaskPriority};
