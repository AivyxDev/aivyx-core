use std::path::Path;

use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
use rand::RngCore;
use redb::{Database, ReadableTable, TableDefinition, TableError};

use aivyx_core::{AivyxError, Result};

use crate::master_key::MasterKey;

const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("secrets");

/// Result of a key re-encryption operation.
#[derive(Debug, Clone)]
pub struct ReEncryptResult {
    /// Number of keys successfully re-encrypted.
    pub keys_migrated: usize,
    /// Error messages for keys that failed to re-encrypt.
    pub errors: Vec<String>,
}

/// An encrypted key-value store backed by redb.
///
/// Each value is encrypted with ChaCha20-Poly1305 using the master key.
/// Storage format per value: `[12-byte nonce][ciphertext+tag]`.
pub struct EncryptedStore {
    db: Database,
}

impl EncryptedStore {
    /// Open or create an encrypted store at the given path.
    ///
    /// Uses a repair callback to handle databases that weren't cleanly closed
    /// (common in Docker containers where advisory file locks persist across restarts).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::builder()
            .set_repair_callback(|progress| {
                eprintln!(
                    "  [warn] repairing store: {:.0}% complete",
                    progress.progress() * 100.0
                );
            })
            .create(path.as_ref())
            .map_err(|e| AivyxError::Storage(format!("failed to open store: {e}")))?;
        Ok(Self { db })
    }

    /// Store a value, encrypting it under the master key.
    pub fn put(&self, key: &str, plaintext: &[u8], master_key: &MasterKey) -> Result<()> {
        let encrypted = self.encrypt(plaintext, master_key)?;

        let txn = self
            .db
            .begin_write()
            .map_err(|e| AivyxError::Storage(format!("write txn failed: {e}")))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| AivyxError::Storage(format!("open table failed: {e}")))?;
            table
                .insert(key, encrypted.as_slice())
                .map_err(|e| AivyxError::Storage(format!("insert failed: {e}")))?;
        }
        txn.commit()
            .map_err(|e| AivyxError::Storage(format!("commit failed: {e}")))?;

        Ok(())
    }

    /// Retrieve and decrypt a value. Returns `None` if the key does not exist.
    pub fn get(&self, key: &str, master_key: &MasterKey) -> Result<Option<Vec<u8>>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Storage(format!("read txn failed: {e}")))?;
        let table = match txn.open_table(TABLE) {
            Ok(t) => t,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(AivyxError::Storage(format!("open table failed: {e}"))),
        };

        let value = table
            .get(key)
            .map_err(|e| AivyxError::Storage(format!("get failed: {e}")))?;

        match value {
            Some(guard) => {
                let bytes = guard.value();
                let plaintext = self.decrypt(bytes, master_key)?;
                Ok(Some(plaintext))
            }
            None => Ok(None),
        }
    }

    /// Delete a key from the store.
    pub fn delete(&self, key: &str) -> Result<()> {
        let txn = self
            .db
            .begin_write()
            .map_err(|e| AivyxError::Storage(format!("write txn failed: {e}")))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| AivyxError::Storage(format!("open table failed: {e}")))?;
            table
                .remove(key)
                .map_err(|e| AivyxError::Storage(format!("remove failed: {e}")))?;
        }
        txn.commit()
            .map_err(|e| AivyxError::Storage(format!("commit failed: {e}")))?;

        Ok(())
    }

    /// List all keys in the store.
    pub fn list_keys(&self) -> Result<Vec<String>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Storage(format!("read txn failed: {e}")))?;
        let table = match txn.open_table(TABLE) {
            Ok(t) => t,
            Err(TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(AivyxError::Storage(format!("open table failed: {e}"))),
        };

        let mut keys = Vec::new();
        for entry in table
            .iter()
            .map_err(|e| AivyxError::Storage(format!("iter failed: {e}")))?
        {
            let (k, _v) = entry.map_err(|e| AivyxError::Storage(format!("entry failed: {e}")))?;
            keys.push(k.value().to_owned());
        }
        Ok(keys)
    }

    /// Re-encrypt all values in the store from an old key to a new key.
    ///
    /// Operates in two phases to be as atomic as possible:
    /// 1. Decrypt all values with `old_key` (read-only phase).
    /// 2. Re-encrypt and write all pairs in a **single** write transaction.
    ///
    /// If the write transaction commits successfully, the store is fully
    /// migrated. Failures in the read phase are collected and reported; they
    /// do not prevent the remaining successfully-decrypted pairs from being
    /// written. Returns the count of migrated keys and any per-key errors.
    pub fn re_encrypt_all(
        &self,
        old_key: &MasterKey,
        new_key: &MasterKey,
    ) -> Result<ReEncryptResult> {
        let keys = self.list_keys()?;
        let mut errors = Vec::new();

        // Phase 1: Read and decrypt all values with the old key.
        let mut pairs: Vec<(String, Vec<u8>)> = Vec::new();
        for key in &keys {
            match self.get(key, old_key) {
                Ok(Some(plaintext)) => pairs.push((key.clone(), plaintext)),
                Ok(None) => errors.push(format!("{key}: key listed but value missing")),
                Err(e) => errors.push(format!("{key}: decrypt failed: {e}")),
            }
        }

        // Phase 2: Re-encrypt and write all pairs in a single transaction.
        let mut migrated = 0;
        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| AivyxError::Storage(format!("begin write failed: {e}")))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AivyxError::Storage(format!("open table failed: {e}")))?;
            for (key, plaintext) in &pairs {
                match self.encrypt(plaintext, new_key) {
                    Ok(encrypted) => {
                        table
                            .insert(key.as_str(), encrypted.as_slice())
                            .map_err(|e| {
                                AivyxError::Storage(format!("{key}: write failed: {e}"))
                            })?;
                        migrated += 1;
                    }
                    Err(e) => errors.push(format!("{key}: re-encrypt failed: {e}")),
                }
            }
        }
        write_txn
            .commit()
            .map_err(|e| AivyxError::Storage(format!("commit failed: {e}")))?;

        Ok(ReEncryptResult {
            keys_migrated: migrated,
            errors,
        })
    }

    fn encrypt(&self, plaintext: &[u8], master_key: &MasterKey) -> Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new(master_key.expose_secret().into());

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = chacha20poly1305::Nonce::from(nonce_bytes);

        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| AivyxError::Crypto(format!("encryption failed: {e}")))?;

        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    fn decrypt(&self, data: &[u8], master_key: &MasterKey) -> Result<Vec<u8>> {
        if data.len() < 12 {
            return Err(AivyxError::Crypto("ciphertext too short".into()));
        }

        let (nonce_bytes, ciphertext) = data.split_at(12);
        let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);

        let cipher = ChaCha20Poly1305::new(master_key.expose_secret().into());
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| AivyxError::Crypto("decryption failed".into()))?;

        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_store() -> (EncryptedStore, MasterKey, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("aivyx-test-{}", rand::random::<u64>()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.redb");
        let store = EncryptedStore::open(&path).unwrap();
        let key = MasterKey::generate();
        (store, key, dir)
    }

    #[test]
    fn put_get_round_trip() {
        let (store, key, dir) = temp_store();
        store.put("api-key", b"sk-secret-123", &key).unwrap();
        let got = store.get("api-key", &key).unwrap().unwrap();
        assert_eq!(got, b"sk-secret-123");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wrong_key_fails() {
        let (store, key, dir) = temp_store();
        store.put("api-key", b"secret", &key).unwrap();

        let wrong_key = MasterKey::generate();
        let result = store.get("api-key", &wrong_key);
        assert!(result.is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_missing_returns_none() {
        let (store, key, dir) = temp_store();
        let got = store.get("nonexistent", &key).unwrap();
        assert!(got.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_works() {
        let (store, key, dir) = temp_store();
        store.put("key1", b"val1", &key).unwrap();
        store.delete("key1").unwrap();
        let got = store.get("key1", &key).unwrap();
        assert!(got.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn re_encrypt_all_round_trip() {
        let (store, old_key, dir) = temp_store();
        store.put("key1", b"secret1", &old_key).unwrap();
        store.put("key2", b"secret2", &old_key).unwrap();

        let new_key = MasterKey::generate();
        let result = store.re_encrypt_all(&old_key, &new_key).unwrap();

        assert_eq!(result.keys_migrated, 2);
        assert!(result.errors.is_empty());

        // Verify new key works
        assert_eq!(store.get("key1", &new_key).unwrap().unwrap(), b"secret1");
        assert_eq!(store.get("key2", &new_key).unwrap().unwrap(), b"secret2");

        // Old key should fail
        assert!(store.get("key1", &old_key).is_err());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn re_encrypt_with_wrong_old_key_fails() {
        let (store, real_key, dir) = temp_store();
        store.put("secret", b"data", &real_key).unwrap();

        let wrong_key = MasterKey::generate();
        let new_key = MasterKey::generate();
        let result = store.re_encrypt_all(&wrong_key, &new_key).unwrap();

        assert_eq!(result.keys_migrated, 0);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("decrypt failed"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_keys_works() {
        let (store, key, dir) = temp_store();
        store.put("alpha", b"a", &key).unwrap();
        store.put("beta", b"b", &key).unwrap();
        store.put("gamma", b"c", &key).unwrap();

        let mut keys = store.list_keys().unwrap();
        keys.sort();
        assert_eq!(keys, vec!["alpha", "beta", "gamma"]);
        fs::remove_dir_all(&dir).ok();
    }
}
