use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use aivyx_core::{AivyxError, Result};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::event::AuditEvent;

type HmacSha256 = Hmac<Sha256>;

/// A single entry in the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Zero-based position in the log.
    pub sequence_number: u64,
    /// RFC 3339 timestamp of when the entry was recorded.
    pub timestamp: String,
    /// The audited event.
    pub event: AuditEvent,
    /// Hex-encoded HMAC-SHA256 chaining this entry to its predecessor.
    pub hmac: String,
}

/// Result of verifying the HMAC chain.
#[derive(Debug, Clone)]
pub struct VerifyResult {
    /// Number of entries that were verified.
    pub entries_checked: u64,
    /// Whether the entire HMAC chain is intact.
    pub valid: bool,
}

/// Append-only, HMAC-chained audit log backed by a file.
///
/// Each line in the file is a JSON-serialized `AuditEntry`. The HMAC chain
/// ensures that any tampering with past entries is detectable.
pub struct AuditLog {
    path: PathBuf,
    key: Zeroizing<Vec<u8>>,
    /// Cached last (sequence_number, hmac_bytes) to avoid re-reading the file.
    last_state: Mutex<Option<(u64, Vec<u8>)>>,
}

impl AuditLog {
    /// Create or open an audit log at the given path.
    pub fn new(path: impl Into<PathBuf>, key: &[u8]) -> Self {
        Self {
            path: path.into(),
            key: Zeroizing::new(key.to_vec()),
            last_state: Mutex::new(None),
        }
    }

    /// Append an event to the log.
    pub fn append(&self, event: AuditEvent) -> Result<()> {
        let mut cache = self
            .last_state
            .lock()
            .map_err(|_| AivyxError::Other("audit log cache lock poisoned".into()))?;

        let (seq, prev_hmac_bytes) = match cache.as_ref() {
            Some((last_seq, last_hmac)) => (last_seq + 1, last_hmac.clone()),
            None => {
                // First append in this process — read the file to bootstrap the cache
                let entries = self.read_all()?;
                if let Some(last) = entries.last() {
                    let hmac_bytes = hex::decode(&last.hmac).map_err(|e| {
                        AivyxError::AuditIntegrity(format!("corrupt HMAC hex: {e}"))
                    })?;
                    (last.sequence_number + 1, hmac_bytes)
                } else {
                    (0, vec![0u8; 32])
                }
            }
        };

        let timestamp = Utc::now().to_rfc3339();

        let mut entry = AuditEntry {
            sequence_number: seq,
            timestamp,
            event,
            hmac: String::new(),
        };

        let json_for_hmac = serde_json::to_vec(&entry)?;

        let mut mac = HmacSha256::new_from_slice(&self.key)
            .map_err(|e| AivyxError::Crypto(format!("HMAC key error: {e}")))?;
        mac.update(&prev_hmac_bytes);
        mac.update(&json_for_hmac);
        let hmac_bytes = mac.finalize().into_bytes();
        let hmac_vec = hmac_bytes.to_vec();
        entry.hmac = hex::encode(&hmac_vec);

        let line = serde_json::to_string(&entry)?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        writeln!(file, "{line}")?;

        // Update the cache
        *cache = Some((seq, hmac_vec));

        Ok(())
    }

    /// Verify the full HMAC chain. Returns how many entries were checked and
    /// whether the chain is intact.
    pub fn verify(&self) -> Result<VerifyResult> {
        let entries = self.read_all()?;

        if entries.is_empty() {
            return Ok(VerifyResult {
                entries_checked: 0,
                valid: true,
            });
        }

        // Bootstrap: the first entry chains from a deterministic zero HMAC.
        // This matches what append() uses for sequence 0, so any tampering
        // of the first entry (or insertion before it) will break the chain.
        let mut prev_hmac_bytes = vec![0u8; 32];

        for (i, entry) in entries.iter().enumerate() {
            let mut check_entry = entry.clone();
            check_entry.hmac = String::new();

            let json_for_hmac = serde_json::to_vec(&check_entry)?;

            let mut mac = HmacSha256::new_from_slice(&self.key)
                .map_err(|e| AivyxError::Crypto(format!("HMAC key error: {e}")))?;
            mac.update(&prev_hmac_bytes);
            mac.update(&json_for_hmac);
            let expected = hex::encode(mac.finalize().into_bytes());

            if entry.hmac != expected {
                return Ok(VerifyResult {
                    entries_checked: i as u64 + 1,
                    valid: false,
                });
            }

            prev_hmac_bytes = hex::decode(&entry.hmac).map_err(|e| {
                AivyxError::AuditIntegrity(format!("corrupt HMAC hex at entry {i}: {e}"))
            })?;
        }

        Ok(VerifyResult {
            entries_checked: entries.len() as u64,
            valid: true,
        })
    }

    /// Return the last `n` entries (or fewer if the log is shorter).
    pub fn recent(&self, n: usize) -> Result<Vec<AuditEntry>> {
        let entries = self.read_all()?;
        let start = entries.len().saturating_sub(n);
        Ok(entries[start..].to_vec())
    }

    /// Total number of entries in the log.
    pub fn len(&self) -> Result<usize> {
        let entries = self.read_all()?;
        Ok(entries.len())
    }

    /// Whether the log file is empty or missing.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Read all entries from the log.
    ///
    /// Used by export, search, and retention modules. Callers should prefer
    /// the higher-level `recent()`, `search()`, or `verify()` methods for
    /// most use cases.
    pub fn read_all_entries(&self) -> Result<Vec<AuditEntry>> {
        self.read_all()
    }

    /// Return a reference to the log file path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Return a reference to the HMAC key.
    pub(crate) fn hmac_key(&self) -> &[u8] {
        &self.key
    }

    // ── internal ──────────────────────────────────────────────

    fn read_all(&self) -> Result<Vec<AuditEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for (i, line) in reader.lines().enumerate() {
            let line: String = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(trimmed).map_err(|e| {
                AivyxError::AuditIntegrity(format!("malformed entry at line {i}: {e}"))
            })?;
            entries.push(entry);
        }

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AuditEvent;
    use aivyx_core::{AgentId, AutonomyTier};
    fn tmp_path() -> PathBuf {
        let name = format!("aivyx_audit_test_{}.jsonl", uuid::Uuid::new_v4());
        std::env::temp_dir().join(name)
    }

    #[test]
    fn append_and_verify() {
        let path = tmp_path();
        let key = b"test-key-32-bytes-long-enough!!!";
        let log = AuditLog::new(&path, key);

        log.append(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        })
        .unwrap();
        log.append(AuditEvent::AgentCreated {
            agent_id: AgentId::new(),
            autonomy_tier: AutonomyTier::Leash,
        })
        .unwrap();

        let result = log.verify().unwrap();
        assert!(result.valid);
        assert_eq!(result.entries_checked, 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tampered_entry_fails_verification() {
        let path = tmp_path();
        let key = b"test-key-32-bytes-long-enough!!!";
        let log = AuditLog::new(&path, key);

        log.append(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        })
        .unwrap();
        log.append(AuditEvent::AgentDestroyed {
            agent_id: AgentId::new(),
        })
        .unwrap();

        // Tamper: modify the first entry's timestamp (breaks its HMAC).
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        let mut entry: AuditEntry = serde_json::from_str(&lines[0]).unwrap();
        entry.timestamp = "2000-01-01T00:00:00Z".to_string();
        lines[0] = serde_json::to_string(&entry).unwrap();

        let mut file = File::create(&path).unwrap();
        for line in &lines {
            writeln!(file, "{line}").unwrap();
        }

        let result = log.verify().unwrap();
        assert!(!result.valid);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn recent_returns_last_n() {
        let path = tmp_path();
        let key = b"recent-test-key!!!!!!!!!!!!!!!!";
        let log = AuditLog::new(&path, key);

        for _ in 0..5 {
            log.append(AuditEvent::SystemInit {
                timestamp: Utc::now(),
            })
            .unwrap();
        }

        let last_two = log.recent(2).unwrap();
        assert_eq!(last_two.len(), 2);
        assert_eq!(last_two[0].sequence_number, 3);
        assert_eq!(last_two[1].sequence_number, 4);

        let all = log.recent(100).unwrap();
        assert_eq!(all.len(), 5);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn empty_log_verifies() {
        let path = tmp_path();
        let key = b"empty-test-key!!!!!!!!!!!!!!!!!!";
        let log = AuditLog::new(&path, key);

        let result = log.verify().unwrap();
        assert!(result.valid);
        assert_eq!(result.entries_checked, 0);
    }

    #[test]
    fn hmac_chain_is_deterministic() {
        let key = b"deterministic-key!!!!!!!!!!!!!!!!";

        let path1 = tmp_path();
        let path2 = tmp_path();

        // Build identical entries by hand (bypassing Utc::now()) to prove determinism.
        let entry_json = r#"{"sequence_number":0,"timestamp":"2025-01-01T00:00:00Z","event":{"type":"SystemInit","timestamp":"2025-01-01T00:00:00Z"},"hmac":""}"#;
        let entry_bytes = entry_json.as_bytes();

        let prev = vec![0u8; 32];
        let mut mac = HmacSha256::new_from_slice(key.as_slice()).unwrap();
        mac.update(&prev);
        mac.update(entry_bytes);
        let hmac_hex = hex::encode(mac.finalize().into_bytes());

        let full_entry = format!(
            r#"{{"sequence_number":0,"timestamp":"2025-01-01T00:00:00Z","event":{{"type":"SystemInit","timestamp":"2025-01-01T00:00:00Z"}},"hmac":"{hmac_hex}"}}"#
        );

        std::fs::write(&path1, format!("{full_entry}\n")).unwrap();
        std::fs::write(&path2, format!("{full_entry}\n")).unwrap();

        let log1 = AuditLog::new(&path1, key);
        let log2 = AuditLog::new(&path2, key);

        assert!(log1.verify().unwrap().valid);
        assert!(log2.verify().unwrap().valid);

        let e1 = log1.recent(1).unwrap();
        let e2 = log2.recent(1).unwrap();
        assert_eq!(e1[0].hmac, e2[0].hmac);

        std::fs::remove_file(&path1).ok();
        std::fs::remove_file(&path2).ok();
    }

    #[test]
    fn len_counts_entries() {
        let path = tmp_path();
        let key = b"len-test-key!!!!!!!!!!!!!!!!!!!";
        let log = AuditLog::new(&path, key);

        assert_eq!(log.len().unwrap(), 0);

        log.append(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        })
        .unwrap();
        assert_eq!(log.len().unwrap(), 1);

        log.append(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        })
        .unwrap();
        assert_eq!(log.len().unwrap(), 2);

        std::fs::remove_file(&path).ok();
    }
}
