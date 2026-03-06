use argon2::{Algorithm, Argon2, Version};
use serde::{Deserialize, Serialize};

use aivyx_core::Result;

/// Parameters for argon2id key derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory cost in KiB (default: 64 MiB).
    pub memory_cost_kib: u32,
    /// Number of iterations (default: 3).
    pub time_cost: u32,
    /// Degree of parallelism (default: 1).
    pub parallelism: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            memory_cost_kib: 64 * 1024, // 64 MiB
            time_cost: 3,
            parallelism: 1,
        }
    }
}

/// Derive a 32-byte key from a passphrase and salt using argon2id.
pub fn derive_key(passphrase: &[u8], salt: &[u8], params: &KdfParams) -> Result<[u8; 32]> {
    let argon2_params = argon2::Params::new(
        params.memory_cost_kib,
        params.time_cost,
        params.parallelism,
        Some(32),
    )
    .map_err(|e| aivyx_core::AivyxError::Crypto(format!("invalid KDF params: {e}")))?;

    let hasher = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params);

    let mut output = [0u8; 32];
    hasher
        .hash_password_into(passphrase, salt, &mut output)
        .map_err(|e| aivyx_core::AivyxError::Crypto(format!("KDF failed: {e}")))?;

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_output() {
        let params = KdfParams {
            memory_cost_kib: 256,
            time_cost: 1,
            parallelism: 1,
        };
        let pass = b"test-passphrase";
        let salt = b"sixteen-byte-sal"; // 16 bytes

        let k1 = derive_key(pass, salt, &params).unwrap();
        let k2 = derive_key(pass, salt, &params).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn different_salt_different_key() {
        let params = KdfParams {
            memory_cost_kib: 256,
            time_cost: 1,
            parallelism: 1,
        };
        let pass = b"test-passphrase";

        let k1 = derive_key(pass, b"salt-aaaaaaaaaa00", &params).unwrap();
        let k2 = derive_key(pass, b"salt-bbbbbbbbbb00", &params).unwrap();
        assert_ne!(k1, k2);
    }
}
