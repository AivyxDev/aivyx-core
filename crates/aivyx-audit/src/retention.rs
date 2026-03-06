//! Audit log retention and pruning.
//!
//! Removes old entries while re-establishing the HMAC chain from scratch
//! to maintain integrity of the remaining entries.

use std::fs::File;
use std::io::Write;

use aivyx_core::{AivyxError, Result};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::event::AuditEvent;
use crate::log::{AuditEntry, AuditLog};

type HmacSha256 = Hmac<Sha256>;

/// Result of a pruning operation.
#[derive(Debug, Clone)]
pub struct PruneResult {
    /// Number of entries that were removed.
    pub entries_removed: usize,
    /// Number of entries remaining after pruning.
    pub entries_remaining: usize,
}

/// Prune audit log entries older than the given cutoff date.
///
/// **This rewrites the HMAC chain from scratch** because removing head entries
/// invalidates the chain. After pruning, a `LogPruned` event is appended.
///
/// # Safety
///
/// This is a destructive operation. The caller should verify the log is intact
/// (via `AuditLog::verify()`) before pruning.
pub fn prune(log: &AuditLog, keep_after: DateTime<Utc>) -> Result<PruneResult> {
    let all_entries = log.read_all_entries()?;
    let key = log.hmac_key();

    // Partition entries by date
    let (old, recent): (Vec<_>, Vec<_>) = all_entries.into_iter().partition(|entry| {
        entry
            .timestamp
            .parse::<DateTime<Utc>>()
            .map(|ts| ts < keep_after)
            .unwrap_or(false)
    });

    let entries_removed = old.len();

    if entries_removed == 0 {
        return Ok(PruneResult {
            entries_removed: 0,
            entries_remaining: recent.len(),
        });
    }

    // Re-chain the remaining entries from scratch
    let rechained = rechain_entries(&recent, key)?;

    // Write the rechained entries to a temp file, then rename
    let temp_path = log.path().with_extension("jsonl.tmp");
    {
        let mut file = File::create(&temp_path).map_err(|e| {
            AivyxError::AuditIntegrity(format!("failed to create temp prune file: {e}"))
        })?;
        for entry in &rechained {
            let line = serde_json::to_string(entry)?;
            writeln!(file, "{line}").map_err(|e| {
                AivyxError::AuditIntegrity(format!("failed to write pruned entry: {e}"))
            })?;
        }
        file.flush()
            .map_err(|e| AivyxError::AuditIntegrity(format!("failed to flush pruned log: {e}")))?;
    }

    // Atomic replace (rename on Unix is atomic within same filesystem)
    std::fs::rename(&temp_path, log.path())
        .map_err(|e| AivyxError::AuditIntegrity(format!("failed to replace audit log: {e}")))?;

    let entries_remaining = rechained.len();

    // Append a LogPruned event to the new log
    let oldest_remaining = rechained
        .first()
        .map(|e| e.timestamp.clone())
        .unwrap_or_default();

    // Re-open the log to get fresh state with correct HMAC cache
    let fresh_log = AuditLog::new(log.path(), key);
    fresh_log.append(AuditEvent::LogPruned {
        entries_removed,
        oldest_remaining,
    })?;

    Ok(PruneResult {
        entries_removed,
        entries_remaining: entries_remaining + 1, // +1 for LogPruned event
    })
}

/// Re-compute the HMAC chain for a set of entries starting from sequence 0.
fn rechain_entries(entries: &[AuditEntry], key: &[u8]) -> Result<Vec<AuditEntry>> {
    let mut result = Vec::with_capacity(entries.len());
    let mut prev_hmac_bytes = vec![0u8; 32];

    for (i, original) in entries.iter().enumerate() {
        let mut entry = AuditEntry {
            sequence_number: i as u64,
            timestamp: original.timestamp.clone(),
            event: original.event.clone(),
            hmac: String::new(),
        };

        let json_for_hmac = serde_json::to_vec(&entry)?;

        let mut mac = HmacSha256::new_from_slice(key)
            .map_err(|e| AivyxError::Crypto(format!("HMAC key error: {e}")))?;
        mac.update(&prev_hmac_bytes);
        mac.update(&json_for_hmac);
        let hmac_bytes = mac.finalize().into_bytes();
        prev_hmac_bytes = hmac_bytes.to_vec();
        entry.hmac = hex::encode(&prev_hmac_bytes);

        result.push(entry);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::{AgentId, AutonomyTier};

    fn test_log_with_entries(count: usize) -> (AuditLog, std::path::PathBuf) {
        let name = format!("aivyx_retention_test_{}.jsonl", uuid::Uuid::new_v4());
        let path = std::env::temp_dir().join(name);
        let key = b"retention-test-key!!!!!!!!!!!!!!!";
        let log = AuditLog::new(&path, key);

        for _ in 0..count {
            log.append(AuditEvent::AgentCreated {
                agent_id: AgentId::new(),
                autonomy_tier: AutonomyTier::Trust,
            })
            .unwrap();
        }
        (log, path)
    }

    #[test]
    fn prune_removes_old_entries() {
        let (log, path) = test_log_with_entries(5);

        // All entries are "now" — set cutoff in the future to prune everything
        let cutoff = Utc::now() + chrono::Duration::seconds(60);
        let result = prune(&log, cutoff).unwrap();

        assert_eq!(result.entries_removed, 5);
        // Only LogPruned event remains
        assert_eq!(result.entries_remaining, 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn prune_rechain_verifies() {
        let (log, path) = test_log_with_entries(10);

        // Prune entries older than future → removes all, re-chains with LogPruned
        let cutoff = Utc::now() + chrono::Duration::seconds(60);
        prune(&log, cutoff).unwrap();

        // Verify the new chain is intact
        let fresh = AuditLog::new(&path, b"retention-test-key!!!!!!!!!!!!!!!");
        let verify_result = fresh.verify().unwrap();
        assert!(verify_result.valid);
        assert_eq!(verify_result.entries_checked, 1); // only LogPruned

        std::fs::remove_file(&path).ok();
    }
}
