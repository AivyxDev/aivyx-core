//! Cryptographic primitives for the aivyx framework.
//!
//! Provides passphrase-based key derivation (argon2id), a master encryption key
//! with a ChaCha20-Poly1305 envelope, and an encrypted key-value store backed
//! by [`redb`]. Secrets are wrapped in [`secrecy::SecretBox`] and zeroized on
//! drop.

pub mod kdf;
pub mod master_key;
pub mod secret;
pub mod store;

pub use kdf::{KdfParams, derive_key};
pub use master_key::{
    MasterKey, MasterKeyEnvelope, derive_audit_key, derive_memory_key, derive_schedule_key,
    derive_task_key, derive_team_session_key, derive_tool_key,
};
pub use store::{EncryptedStore, ReEncryptResult};
