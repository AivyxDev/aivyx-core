//! Core types and traits for the aivyx agent framework.
//!
//! This crate provides the foundational building blocks shared across all aivyx
//! crates: identity types, error handling, autonomy tiers, principal
//! identification, and the [`Tool`] and [`ChannelAdapter`] traits.

pub mod autonomy;
pub mod cache;
pub mod error;
pub mod id;
pub mod principal;
pub mod progress;
pub mod scope;
pub mod tool_registry;
pub mod traits;

pub use autonomy::AutonomyTier;
pub use cache::Cacheable;
pub use error::{AivyxError, Result, ResultExt};
pub use id::{
    AgentId, CapabilityId, MemoryId, MessageId, NotificationId, SessionId, SkillId, TaskId, ToolId,
    TripleId,
};
pub use principal::Principal;
pub use progress::{ChannelProgressSink, NoopProgressSink, ProgressSink};
pub use scope::CapabilityScope;
pub use tool_registry::ToolRegistry;
pub use traits::{ChannelAdapter, Tool};
