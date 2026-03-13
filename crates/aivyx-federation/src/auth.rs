//! Ed25519 authentication for federation peer-to-peer communication.
//!
//! Each instance has a keypair. Outgoing requests are signed with the private key.
//! Incoming requests are verified against the peer's known public key.
//! Signatures include a timestamp to prevent replay attacks.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use aivyx_core::AivyxError;

/// Maximum age of a signed request before it's considered stale (60 seconds).
const MAX_REQUEST_AGE_SECS: u64 = 60;

/// A signed federation request header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedHeader {
    /// The instance ID of the sender.
    pub instance_id: String,
    /// Unix timestamp when the request was signed.
    pub timestamp: u64,
    /// Base64-encoded Ed25519 signature of "{instance_id}:{timestamp}:{body_hash}".
    pub signature: String,
}

/// Manages Ed25519 keypair for federation auth.
pub struct FederationAuth {
    instance_id: String,
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl FederationAuth {
    /// Create from an existing signing key.
    pub fn new(instance_id: String, signing_key: SigningKey) -> Self {
        let verifying_key = signing_key.verifying_key();
        Self {
            instance_id,
            signing_key,
            verifying_key,
        }
    }

    /// Generate a new random keypair.
    pub fn generate(instance_id: String) -> Self {
        let mut rng = rand::thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        Self::new(instance_id, signing_key)
    }

    /// Load a signing key from a file, or generate + save if absent.
    pub fn load_or_generate(instance_id: String, key_path: &Path) -> Result<Self, AivyxError> {
        if key_path.exists() {
            let bytes = std::fs::read(key_path)
                .map_err(|e| AivyxError::Other(format!("read federation key: {e}")))?;
            if bytes.len() != 32 {
                return Err(AivyxError::Other(
                    "federation key must be exactly 32 bytes".into(),
                ));
            }
            let key_bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| AivyxError::Other("invalid key length".into()))?;
            let signing_key = SigningKey::from_bytes(&key_bytes);
            Ok(Self::new(instance_id, signing_key))
        } else {
            let auth = Self::generate(instance_id);
            // Ensure parent directory exists
            if let Some(parent) = key_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| AivyxError::Other(format!("create key dir: {e}")))?;
            }
            std::fs::write(key_path, auth.signing_key.to_bytes())
                .map_err(|e| AivyxError::Other(format!("write federation key: {e}")))?;
            tracing::info!("Generated new federation keypair at {}", key_path.display());
            Ok(auth)
        }
    }

    /// Get this instance's public key as base64 (for sharing with peers).
    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.verifying_key.as_bytes())
    }

    /// Sign a request body, producing a `SignedHeader`.
    pub fn sign_request(&self, body: &[u8]) -> SignedHeader {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let body_hash = sha256_hex(body);
        let message = format!("{}:{}:{}", self.instance_id, timestamp, body_hash);
        let signature = self.signing_key.sign(message.as_bytes());

        SignedHeader {
            instance_id: self.instance_id.clone(),
            timestamp,
            signature: BASE64.encode(signature.to_bytes()),
        }
    }

    /// Verify a signed request from a peer.
    pub fn verify_request(
        peer_public_key: &str,
        header: &SignedHeader,
        body: &[u8],
    ) -> Result<(), AivyxError> {
        // Check timestamp freshness
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now.saturating_sub(header.timestamp) > MAX_REQUEST_AGE_SECS {
            return Err(AivyxError::Other("federation request expired".into()));
        }

        // Decode public key
        let key_bytes = BASE64
            .decode(peer_public_key)
            .map_err(|e| AivyxError::Other(format!("invalid peer public key: {e}")))?;
        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| AivyxError::Other("peer public key must be 32 bytes".into()))?;
        let verifying_key = VerifyingKey::from_bytes(&key_array)
            .map_err(|e| AivyxError::Other(format!("invalid Ed25519 key: {e}")))?;

        // Decode signature
        let sig_bytes = BASE64
            .decode(&header.signature)
            .map_err(|e| AivyxError::Other(format!("invalid signature encoding: {e}")))?;
        let sig_array: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| AivyxError::Other("signature must be 64 bytes".into()))?;
        let signature = ed25519_dalek::Signature::from_bytes(&sig_array);

        // Verify
        let body_hash = sha256_hex(body);
        let message = format!("{}:{}:{}", header.instance_id, header.timestamp, body_hash);

        verifying_key
            .verify(message.as_bytes(), &signature)
            .map_err(|_| AivyxError::Other("federation signature verification failed".into()))
    }

    /// Load a signing key from an encrypted file, or generate + encrypt + save if absent.
    ///
    /// The `wrapping_key` must be exactly 32 bytes (e.g., derived via HKDF from a master key).
    /// The file format is: `nonce (12 bytes) || ciphertext (48 bytes)` = 60 bytes total.
    pub fn load_or_generate_encrypted(
        instance_id: String,
        key_path: &Path,
        wrapping_key: &[u8; 32],
    ) -> Result<Self, AivyxError> {
        use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};

        if key_path.exists() {
            let data = std::fs::read(key_path)
                .map_err(|e| AivyxError::Other(format!("read federation key: {e}")))?;

            if data.len() == 32 {
                // Unencrypted legacy file — load it, re-encrypt, and save.
                tracing::warn!(
                    "Federation key at {} is unencrypted — re-encrypting at rest",
                    key_path.display()
                );
                let signing_key = SigningKey::from_bytes(
                    &data
                        .try_into()
                        .map_err(|_| AivyxError::Other("invalid key length".into()))?,
                );
                let auth = Self::new(instance_id, signing_key);
                auth.save_encrypted(key_path, wrapping_key)?;
                return Ok(auth);
            }

            // Encrypted format: 12-byte nonce + 48-byte ciphertext (32 key + 16 AEAD tag)
            if data.len() != 60 {
                return Err(AivyxError::Other(format!(
                    "federation key file has unexpected size {} (expected 60 for encrypted or 32 for legacy)",
                    data.len()
                )));
            }

            let nonce = chacha20poly1305::Nonce::from_slice(&data[..12]);
            let cipher = ChaCha20Poly1305::new(wrapping_key.into());
            let plaintext = cipher
                .decrypt(nonce, &data[12..])
                .map_err(|_| AivyxError::Crypto("federation key decryption failed".into()))?;

            let key_bytes: [u8; 32] = plaintext
                .try_into()
                .map_err(|_| AivyxError::Other("decrypted key is not 32 bytes".into()))?;

            Ok(Self::new(instance_id, SigningKey::from_bytes(&key_bytes)))
        } else {
            let auth = Self::generate(instance_id);
            auth.save_encrypted(key_path, wrapping_key)?;
            tracing::info!(
                "Generated new encrypted federation keypair at {}",
                key_path.display()
            );
            Ok(auth)
        }
    }

    /// Encrypt and write the signing key to disk.
    fn save_encrypted(&self, key_path: &Path, wrapping_key: &[u8; 32]) -> Result<(), AivyxError> {
        use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};

        if let Some(parent) = key_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AivyxError::Other(format!("create key dir: {e}")))?;
        }

        let mut nonce_bytes = [0u8; 12];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce_bytes);
        let nonce = chacha20poly1305::Nonce::from(nonce_bytes);

        let cipher = ChaCha20Poly1305::new(wrapping_key.into());
        let ciphertext = cipher
            .encrypt(&nonce, self.signing_key.to_bytes().as_ref())
            .map_err(|e| AivyxError::Crypto(format!("federation key encryption failed: {e}")))?;

        let mut file_data = Vec::with_capacity(12 + ciphertext.len());
        file_data.extend_from_slice(&nonce_bytes);
        file_data.extend_from_slice(&ciphertext);

        std::fs::write(key_path, &file_data)
            .map_err(|e| AivyxError::Other(format!("write encrypted federation key: {e}")))?;

        Ok(())
    }

    /// Get the instance ID.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

/// Simple SHA-256 hex hash.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    hex::encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let auth = FederationAuth::generate("test-instance".to_string());
        let body = b"hello federation";
        let header = auth.sign_request(body);

        let pub_key = auth.public_key_base64();
        FederationAuth::verify_request(&pub_key, &header, body).expect("verification should pass");
    }

    #[test]
    fn reject_tampered_body() {
        let auth = FederationAuth::generate("test-instance".to_string());
        let header = auth.sign_request(b"original body");

        let pub_key = auth.public_key_base64();
        let result = FederationAuth::verify_request(&pub_key, &header, b"tampered body");
        assert!(result.is_err());
    }

    #[test]
    fn reject_expired_request() {
        let auth = FederationAuth::generate("test-instance".to_string());
        let mut header = auth.sign_request(b"body");
        header.timestamp -= MAX_REQUEST_AGE_SECS + 10; // expired

        let pub_key = auth.public_key_base64();
        let result = FederationAuth::verify_request(&pub_key, &header, b"body");
        assert!(result.is_err());
    }

    #[test]
    fn reject_wrong_peers_key() {
        let auth_a = FederationAuth::generate("instance-a".to_string());
        let auth_b = FederationAuth::generate("instance-b".to_string());
        let body = b"secret message";

        let header = auth_a.sign_request(body);
        let wrong_pub = auth_b.public_key_base64();

        // Verifying with a different peer's key must fail
        let result = FederationAuth::verify_request(&wrong_pub, &header, body);
        assert!(result.is_err());
    }

    #[test]
    fn sign_request_structure() {
        let auth = FederationAuth::generate("struct-test".to_string());
        let header = auth.sign_request(b"payload");

        assert_eq!(header.instance_id, "struct-test");

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(header.timestamp <= now && header.timestamp >= now - 2);

        // Ed25519 signatures are exactly 64 bytes
        let sig_bytes = BASE64.decode(&header.signature).unwrap();
        assert_eq!(sig_bytes.len(), 64);
    }

    #[test]
    fn verify_empty_body() {
        let auth = FederationAuth::generate("empty-body".to_string());
        let header = auth.sign_request(b"");

        let pub_key = auth.public_key_base64();
        FederationAuth::verify_request(&pub_key, &header, b"").expect("empty body should verify");
    }

    #[test]
    fn encrypted_key_roundtrip() {
        let dir = std::env::temp_dir().join(format!("fed-enc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("federation.key");
        let wrapping_key = [0x42u8; 32];

        // Generate + save encrypted
        let auth =
            FederationAuth::load_or_generate_encrypted("enc-test".into(), &key_path, &wrapping_key)
                .unwrap();
        let pub_key = auth.public_key_base64();

        // File should be 60 bytes (12 nonce + 48 ciphertext)
        let file_data = std::fs::read(&key_path).unwrap();
        assert_eq!(file_data.len(), 60);

        // Reload and verify same keypair
        let auth2 =
            FederationAuth::load_or_generate_encrypted("enc-test".into(), &key_path, &wrapping_key)
                .unwrap();
        assert_eq!(auth2.public_key_base64(), pub_key);

        // Signing still works after reload
        let header = auth2.sign_request(b"test");
        FederationAuth::verify_request(&pub_key, &header, b"test").unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn encrypted_key_wrong_wrapping_key_fails() {
        let dir = std::env::temp_dir().join(format!("fed-enc-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("federation.key");
        let wrapping_key = [0x42u8; 32];
        let wrong_key = [0x99u8; 32];

        FederationAuth::load_or_generate_encrypted("enc-test".into(), &key_path, &wrapping_key)
            .unwrap();

        let result =
            FederationAuth::load_or_generate_encrypted("enc-test".into(), &key_path, &wrong_key);
        assert!(result.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn encrypted_key_migrates_legacy_file() {
        let dir = std::env::temp_dir().join(format!("fed-enc-mig-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("federation.key");
        let wrapping_key = [0x42u8; 32];

        // Write an unencrypted legacy key (32 raw bytes)
        let auth = FederationAuth::generate("migrate-test".into());
        let pub_key = auth.public_key_base64();
        std::fs::write(&key_path, auth.signing_key.to_bytes()).unwrap();
        assert_eq!(std::fs::read(&key_path).unwrap().len(), 32);

        // load_or_generate_encrypted should auto-migrate
        let auth2 = FederationAuth::load_or_generate_encrypted(
            "migrate-test".into(),
            &key_path,
            &wrapping_key,
        )
        .unwrap();
        assert_eq!(auth2.public_key_base64(), pub_key);

        // File should now be encrypted (60 bytes)
        assert_eq!(std::fs::read(&key_path).unwrap().len(), 60);

        // Re-load should still work
        let auth3 = FederationAuth::load_or_generate_encrypted(
            "migrate-test".into(),
            &key_path,
            &wrapping_key,
        )
        .unwrap();
        assert_eq!(auth3.public_key_base64(), pub_key);

        std::fs::remove_dir_all(&dir).ok();
    }
}
