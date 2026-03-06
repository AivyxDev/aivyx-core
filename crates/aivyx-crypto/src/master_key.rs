use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
use hkdf::Hkdf;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use aivyx_core::{AivyxError, Result};

use crate::kdf::{KdfParams, derive_key};

/// A 256-bit master encryption key, zeroized on drop.
pub struct MasterKey {
    inner: SecretBox<[u8]>,
}

impl MasterKey {
    /// Generate a random 256-bit master key.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let boxed: Box<[u8]> = Box::from(bytes.as_slice());
        bytes.zeroize();
        Self {
            inner: SecretBox::new(boxed),
        }
    }

    /// Create a `MasterKey` from raw bytes.
    pub fn from_bytes(mut bytes: [u8; 32]) -> Self {
        let boxed: Box<[u8]> = Box::from(bytes.as_slice());
        bytes.zeroize();
        Self {
            inner: SecretBox::new(boxed),
        }
    }

    /// Access the raw key bytes.
    pub fn expose_secret(&self) -> &[u8] {
        self.inner.expose_secret()
    }

    /// Encrypt this master key under a passphrase, returning a persistable envelope.
    pub fn encrypt_to_envelope(&self, passphrase: &[u8]) -> Result<MasterKeyEnvelope> {
        let params = KdfParams::default();

        let mut salt = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut salt);

        let mut derived = derive_key(passphrase, &salt, &params)?;

        let cipher = ChaCha20Poly1305::new((&derived).into());
        derived.zeroize();

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = chacha20poly1305::Nonce::from(nonce_bytes);

        let ciphertext = cipher
            .encrypt(&nonce, self.expose_secret())
            .map_err(|e| AivyxError::Crypto(format!("encryption failed: {e}")))?;

        Ok(MasterKeyEnvelope {
            salt: salt.to_vec(),
            nonce: nonce_bytes.to_vec(),
            ciphertext,
            kdf_params: params,
        })
    }

    /// Decrypt a master key from an envelope using a passphrase.
    pub fn decrypt_from_envelope(passphrase: &[u8], envelope: &MasterKeyEnvelope) -> Result<Self> {
        let mut derived = derive_key(passphrase, &envelope.salt, &envelope.kdf_params)?;

        let cipher = ChaCha20Poly1305::new((&derived).into());
        derived.zeroize();

        let nonce = chacha20poly1305::Nonce::from_slice(&envelope.nonce);

        let plaintext = Zeroizing::new(
            cipher
                .decrypt(nonce, envelope.ciphertext.as_ref())
                .map_err(|_| AivyxError::Crypto("decryption failed (wrong passphrase?)".into()))?,
        );

        let mut key_bytes = [0u8; 32];
        if plaintext.len() != 32 {
            return Err(AivyxError::Crypto(
                "invalid key length after decryption".into(),
            ));
        }
        key_bytes.copy_from_slice(&plaintext);

        Ok(Self::from_bytes(key_bytes))
    }
}

/// Serializable envelope containing an encrypted master key and its KDF metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasterKeyEnvelope {
    /// Random salt used for key derivation (16 bytes).
    pub salt: Vec<u8>,
    /// ChaCha20-Poly1305 nonce (12 bytes).
    pub nonce: Vec<u8>,
    /// Encrypted master key bytes with authentication tag.
    pub ciphertext: Vec<u8>,
    /// Argon2id parameters used to derive the wrapping key.
    pub kdf_params: KdfParams,
}

/// Derive the HMAC key for audit log chaining from the master key using HKDF-SHA256.
///
/// Uses the domain string `"aivyx-audit-hmac-key"` as the HKDF info parameter
/// for domain separation. The master key bytes serve as the input keying material (IKM).
pub fn derive_audit_key(master_key: &MasterKey) -> Vec<u8> {
    let hk = Hkdf::<Sha256>::new(None, master_key.expose_secret());
    let mut okm = vec![0u8; 32];
    hk.expand(b"aivyx-audit-hmac-key", &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    okm
}

/// Derive a dedicated encryption key for the memory subsystem using HKDF-SHA256.
///
/// Uses the domain string `"aivyx-memory-key"` as the HKDF info parameter
/// for domain separation. Returns a full `MasterKey` suitable for use with `EncryptedStore`.
pub fn derive_memory_key(master_key: &MasterKey) -> MasterKey {
    let hk = Hkdf::<Sha256>::new(None, master_key.expose_secret());
    let mut okm = [0u8; 32];
    hk.expand(b"aivyx-memory-key", &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    MasterKey::from_bytes(okm)
}

/// Derive a dedicated encryption key for the task subsystem using HKDF-SHA256.
///
/// Uses the domain string `"aivyx-task-key"` as the HKDF info parameter
/// for domain separation. Returns a full `MasterKey` suitable for use with `EncryptedStore`.
pub fn derive_task_key(master_key: &MasterKey) -> MasterKey {
    let hk = Hkdf::<Sha256>::new(None, master_key.expose_secret());
    let mut okm = [0u8; 32];
    hk.expand(b"aivyx-task-key", &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    MasterKey::from_bytes(okm)
}

/// Derive a dedicated encryption key for the schedule/notification subsystem using HKDF-SHA256.
///
/// Uses the domain string `"aivyx-schedule-key"` as the HKDF info parameter
/// for domain separation. Returns a full `MasterKey` suitable for use with `EncryptedStore`.
pub fn derive_schedule_key(master_key: &MasterKey) -> MasterKey {
    let hk = Hkdf::<Sha256>::new(None, master_key.expose_secret());
    let mut okm = [0u8; 32];
    hk.expand(b"aivyx-schedule-key", &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    MasterKey::from_bytes(okm)
}

/// Derive a dedicated encryption key for contextual tools (translate, notification, email)
/// using HKDF-SHA256.
///
/// Uses the domain string `"aivyx-tool-key"` as the HKDF info parameter
/// for domain separation. Returns a full `MasterKey` suitable for use with `EncryptedStore`.
pub fn derive_tool_key(master_key: &MasterKey) -> MasterKey {
    let hk = Hkdf::<Sha256>::new(None, master_key.expose_secret());
    let mut okm = [0u8; 32];
    hk.expand(b"aivyx-tool-key", &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    MasterKey::from_bytes(okm)
}

/// Derive a dedicated encryption key for the team session subsystem using HKDF-SHA256.
///
/// Uses the domain string `"aivyx-team-session-key"` as the HKDF info parameter
/// for domain separation. Returns a full `MasterKey` suitable for use with `EncryptedStore`.
pub fn derive_team_session_key(master_key: &MasterKey) -> MasterKey {
    let hk = Hkdf::<Sha256>::new(None, master_key.expose_secret());
    let mut okm = [0u8; 32];
    hk.expand(b"aivyx-team-session-key", &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    MasterKey::from_bytes(okm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = MasterKey::generate();
        let original = key.expose_secret().to_vec();

        let envelope = key.encrypt_to_envelope(b"my-passphrase").unwrap();
        let recovered = MasterKey::decrypt_from_envelope(b"my-passphrase", &envelope).unwrap();

        assert_eq!(original, recovered.expose_secret());
    }

    #[test]
    fn wrong_passphrase_fails() {
        let key = MasterKey::generate();
        let envelope = key.encrypt_to_envelope(b"correct-pass").unwrap();

        let result = MasterKey::decrypt_from_envelope(b"wrong-pass", &envelope);
        assert!(result.is_err());
    }

    #[test]
    fn derive_memory_key_is_deterministic_and_distinct() {
        let master = MasterKey::generate();
        let mem_key1 = derive_memory_key(&master);
        let mem_key2 = derive_memory_key(&master);
        // Deterministic
        assert_eq!(mem_key1.expose_secret(), mem_key2.expose_secret());
        // Differs from master
        assert_ne!(master.expose_secret(), mem_key1.expose_secret());
        // Differs from audit key
        let audit_key = derive_audit_key(&master);
        assert_ne!(mem_key1.expose_secret(), audit_key.as_slice());
    }

    #[test]
    fn derive_task_key_is_deterministic_and_distinct() {
        let master = MasterKey::generate();
        let task_key1 = derive_task_key(&master);
        let task_key2 = derive_task_key(&master);
        // Deterministic
        assert_eq!(task_key1.expose_secret(), task_key2.expose_secret());
        // Differs from master
        assert_ne!(master.expose_secret(), task_key1.expose_secret());
        // Differs from memory key
        let mem_key = derive_memory_key(&master);
        assert_ne!(task_key1.expose_secret(), mem_key.expose_secret());
        // Differs from audit key
        let audit_key = derive_audit_key(&master);
        assert_ne!(task_key1.expose_secret(), audit_key.as_slice());
    }

    #[test]
    fn derive_team_session_key_is_deterministic_and_distinct() {
        let master = MasterKey::generate();
        let ts_key1 = derive_team_session_key(&master);
        let ts_key2 = derive_team_session_key(&master);
        // Deterministic
        assert_eq!(ts_key1.expose_secret(), ts_key2.expose_secret());
        // Differs from master
        assert_ne!(master.expose_secret(), ts_key1.expose_secret());
        // Differs from task key
        let task_key = derive_task_key(&master);
        assert_ne!(ts_key1.expose_secret(), task_key.expose_secret());
        // Differs from schedule key
        let sched_key = derive_schedule_key(&master);
        assert_ne!(ts_key1.expose_secret(), sched_key.expose_secret());
        // Differs from memory key
        let mem_key = derive_memory_key(&master);
        assert_ne!(ts_key1.expose_secret(), mem_key.expose_secret());
        // Differs from audit key
        let audit_key = derive_audit_key(&master);
        assert_ne!(ts_key1.expose_secret(), audit_key.as_slice());
    }

    #[test]
    fn derive_schedule_key_is_deterministic_and_distinct() {
        let master = MasterKey::generate();
        let sched_key1 = derive_schedule_key(&master);
        let sched_key2 = derive_schedule_key(&master);
        // Deterministic
        assert_eq!(sched_key1.expose_secret(), sched_key2.expose_secret());
        // Differs from master
        assert_ne!(master.expose_secret(), sched_key1.expose_secret());
        // Differs from task key
        let task_key = derive_task_key(&master);
        assert_ne!(sched_key1.expose_secret(), task_key.expose_secret());
        // Differs from memory key
        let mem_key = derive_memory_key(&master);
        assert_ne!(sched_key1.expose_secret(), mem_key.expose_secret());
    }

    #[test]
    fn derive_tool_key_is_deterministic_and_distinct() {
        let master = MasterKey::generate();
        let tool_key1 = derive_tool_key(&master);
        let tool_key2 = derive_tool_key(&master);
        // Deterministic
        assert_eq!(tool_key1.expose_secret(), tool_key2.expose_secret());
        // Differs from master
        assert_ne!(master.expose_secret(), tool_key1.expose_secret());
        // Differs from task key
        let task_key = derive_task_key(&master);
        assert_ne!(tool_key1.expose_secret(), task_key.expose_secret());
        // Differs from schedule key
        let sched_key = derive_schedule_key(&master);
        assert_ne!(tool_key1.expose_secret(), sched_key.expose_secret());
        // Differs from memory key
        let mem_key = derive_memory_key(&master);
        assert_ne!(tool_key1.expose_secret(), mem_key.expose_secret());
        // Differs from team session key
        let ts_key = derive_team_session_key(&master);
        assert_ne!(tool_key1.expose_secret(), ts_key.expose_secret());
        // Differs from audit key
        let audit_key = derive_audit_key(&master);
        assert_ne!(tool_key1.expose_secret(), audit_key.as_slice());
    }

    #[test]
    fn envelope_serialization() {
        let key = MasterKey::generate();
        let envelope = key.encrypt_to_envelope(b"pass").unwrap();

        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: MasterKeyEnvelope = serde_json::from_str(&json).unwrap();

        let recovered = MasterKey::decrypt_from_envelope(b"pass", &deserialized).unwrap();
        assert_eq!(key.expose_secret(), recovered.expose_secret());
    }
}
