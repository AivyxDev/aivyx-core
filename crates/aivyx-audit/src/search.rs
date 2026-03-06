//! Filtered search across audit log entries.

use aivyx_core::Result;
use chrono::{DateTime, Utc};

use crate::log::{AuditEntry, AuditLog};

/// Maximum number of results returned by a search query.
const MAX_SEARCH_LIMIT: usize = 1000;

/// Filter criteria for audit log searches.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Filter by event type names (serde tag values, e.g. "SystemInit").
    pub event_types: Option<Vec<String>>,
    /// Only include entries at or after this timestamp.
    pub from: Option<DateTime<Utc>>,
    /// Only include entries at or before this timestamp.
    pub to: Option<DateTime<Utc>>,
    /// Maximum number of results (capped at 1000).
    pub limit: Option<usize>,
}

impl AuditLog {
    /// Search the audit log with the given filter.
    ///
    /// Returns entries matching all specified criteria, up to the limit
    /// (max 1000). Results are returned in chronological order.
    pub fn search(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>> {
        let entries = self.read_all_entries()?;
        let limit = filter
            .limit
            .map(|l| l.min(MAX_SEARCH_LIMIT))
            .unwrap_or(MAX_SEARCH_LIMIT);

        let results: Vec<AuditEntry> = entries
            .into_iter()
            .filter(|entry| {
                // Filter by event type
                if let Some(ref types) = filter.event_types {
                    let event_type = extract_event_type(&entry.event);
                    if !types.iter().any(|t| t == &event_type) {
                        return false;
                    }
                }

                // Filter by timestamp range
                if let Ok(ts) = entry.timestamp.parse::<DateTime<Utc>>() {
                    if let Some(ref from) = filter.from
                        && ts < *from
                    {
                        return false;
                    }
                    if let Some(ref to) = filter.to
                        && ts > *to
                    {
                        return false;
                    }
                }

                true
            })
            .take(limit)
            .collect();

        Ok(results)
    }
}

/// Extract the serde tag ("type" field) from an AuditEvent.
fn extract_event_type(event: &crate::event::AuditEvent) -> String {
    if let Ok(json) = serde_json::to_value(event)
        && let Some(t) = json.get("type").and_then(|v| v.as_str())
    {
        return t.to_string();
    }
    "Unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AuditEvent;
    use aivyx_core::{AgentId, AutonomyTier};

    fn test_log() -> (AuditLog, std::path::PathBuf) {
        let name = format!("aivyx_search_test_{}.jsonl", uuid::Uuid::new_v4());
        let path = std::env::temp_dir().join(name);
        let log = AuditLog::new(&path, b"search-test-key!!!!!!!!!!!!!!!!!");
        log.append(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        })
        .unwrap();
        log.append(AuditEvent::AgentCreated {
            agent_id: AgentId::new(),
            autonomy_tier: AutonomyTier::Leash,
        })
        .unwrap();
        log.append(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        })
        .unwrap();
        (log, path)
    }

    #[test]
    fn search_by_event_type() {
        let (log, path) = test_log();
        let filter = AuditFilter {
            event_types: Some(vec!["SystemInit".into()]),
            ..Default::default()
        };
        let results = log.search(&filter).unwrap();
        assert_eq!(results.len(), 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn search_by_date_range() {
        let (log, path) = test_log();
        let now = Utc::now();
        let filter = AuditFilter {
            from: Some(now - chrono::Duration::seconds(10)),
            to: Some(now + chrono::Duration::seconds(10)),
            ..Default::default()
        };
        let results = log.search(&filter).unwrap();
        assert_eq!(results.len(), 3); // all entries are recent
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn search_limit_capped() {
        let (log, path) = test_log();
        let filter = AuditFilter {
            limit: Some(1),
            ..Default::default()
        };
        let results = log.search(&filter).unwrap();
        assert_eq!(results.len(), 1);

        // Test that limit is capped at MAX_SEARCH_LIMIT
        let filter = AuditFilter {
            limit: Some(5000),
            ..Default::default()
        };
        let results = log.search(&filter).unwrap();
        assert_eq!(results.len(), 3); // only 3 entries, all returned
        std::fs::remove_file(&path).ok();
    }
}
