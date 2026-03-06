//! Aivyx Federation — cross-instance agent communication.
//!
//! Enables agents across separate Aivyx Engine instances to discover each other,
//! exchange messages, delegate tasks, and share knowledge — authenticated via
//! Ed25519 signed requests.

pub mod auth;
pub mod client;
pub mod config;
pub mod relay;
pub mod types;
