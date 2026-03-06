use std::sync::atomic::{AtomicU64, Ordering};

use aivyx_crypto::{EncryptedStore, MasterKey};
use proptest::prelude::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Helper to create a temp directory path for a store.
fn temp_store_path(label: &str) -> std::path::PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let name = format!("aivyx_proptest_crypto_{label}_{pid}_{n}");
    std::env::temp_dir().join(name)
}

proptest! {
    /// Encrypting and then decrypting arbitrary byte sequences through
    /// EncryptedStore must always return the original plaintext.
    #[test]
    fn encrypt_decrypt_roundtrip(plaintext in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let path = temp_store_path("roundtrip");
        let key = MasterKey::from_bytes([42u8; 32]);
        let store = EncryptedStore::open(&path).unwrap();

        store.put("test-key", &plaintext, &key).unwrap();
        let retrieved = store.get("test-key", &key).unwrap();

        prop_assert!(retrieved.is_some());
        prop_assert_eq!(retrieved.unwrap(), plaintext);

        // Clean up
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("lock")).ok();
    }

    /// Two different plaintexts should (with overwhelming probability) produce
    /// different ciphertexts. This validates that the encryption uses unique
    /// nonces and is not deterministic.
    #[test]
    fn different_plaintexts_different_ciphertexts(
        a in proptest::collection::vec(any::<u8>(), 1..512),
        b in proptest::collection::vec(any::<u8>(), 1..512),
    ) {
        // Only test when inputs actually differ.
        prop_assume!(a != b);

        let path = temp_store_path("diff");
        let key = MasterKey::from_bytes([99u8; 32]);
        let store = EncryptedStore::open(&path).unwrap();

        store.put("key-a", &a, &key).unwrap();
        store.put("key-b", &b, &key).unwrap();

        let got_a = store.get("key-a", &key).unwrap().unwrap();
        let got_b = store.get("key-b", &key).unwrap().unwrap();

        // The decrypted values must match their respective inputs.
        prop_assert_eq!(&got_a, &a);
        prop_assert_eq!(&got_b, &b);

        // And they must differ from each other (since inputs differ).
        prop_assert_ne!(got_a, got_b);

        // Clean up
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("lock")).ok();
    }
}
