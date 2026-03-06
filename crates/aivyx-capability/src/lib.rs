//! Capability-based authorization for the aivyx framework.
//!
//! Capabilities are unforgeable tokens that grant specific permissions.
//! New capabilities can only be created by *attenuating* (narrowing) an
//! existing one — it is structurally impossible to broaden authority.
//!
//! Key types: [`CapabilityScope`], [`ActionPattern`], [`Capability`],
//! [`CapabilitySet`].

pub mod pattern;
pub mod scope;
pub mod set;
pub mod token;

pub use pattern::ActionPattern;
pub use scope::CapabilityScope;
pub use set::CapabilitySet;
pub use token::Capability;
