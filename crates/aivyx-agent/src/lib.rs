//! Agent runtime for the aivyx framework.
//!
//! Provides agent profiles (TOML-based customization), a turn-based agent loop
//! that communicates with LLM providers, rate limiting, cost tracking, and
//! built-in tools.

pub mod agent;
pub mod analysis_tools;
pub mod built_in_tools;
pub mod compression;
pub mod cost_tracker;
pub mod digest;
pub mod document_tools;
#[cfg(feature = "federation")]
pub mod federation_tools;
pub mod infrastructure_tools;
pub mod memory_extractor;
pub mod network_tools;
pub mod persona;
pub mod plugin_tools;
pub mod profile;
pub mod rate_limiter;
pub mod self_tools;
pub mod session;
pub mod session_store;
pub mod skill_loader;

pub use agent::Agent;
pub use cost_tracker::CostTracker;
pub use digest::generate_digest;
pub use persona::Persona;
pub use profile::{AgentProfile, ProfileCapability};
pub use rate_limiter::RateLimiter;
pub use session::AgentSession;
pub use session_store::{PersistedSession, SessionMetadata, SessionStore};
