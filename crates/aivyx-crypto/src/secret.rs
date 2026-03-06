//! Re-exports from the [`secrecy`] crate plus a convenience type alias.
//!
//! All secret material in aivyx is wrapped in [`SecretBox`] to ensure
//! zeroization on drop and to prevent accidental logging.

pub use secrecy::{ExposeSecret, SecretBox, SecretString};

/// A secret byte vector. Zeroized on drop.
pub type SecretBytes = SecretBox<[u8]>;
