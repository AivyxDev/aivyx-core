//! Notification queue: background findings stored for next interaction.
//!
//! [`NotificationStore`] wraps [`EncryptedStore`] with a `"notif:{id}"` key
//! namespace. Unrated notifications are transient — drained when the agent
//! builds its system prompt. Rated notifications are preserved as history
//! for the agent feedback loop.

use std::path::Path;

use aivyx_core::{NotificationId, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Human feedback rating for a schedule output.
///
/// Used in the agent feedback loop: humans rate outputs, agents reflect
/// on ratings during periodic self-improvement cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    /// Output was valuable and actionable.
    Useful,
    /// Output had some value but needs improvement.
    Partial,
    /// Output was not useful or relevant.
    Useless,
}

impl std::fmt::Display for Rating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rating::Useful => write!(f, "useful"),
            Rating::Partial => write!(f, "partial"),
            Rating::Useless => write!(f, "useless"),
        }
    }
}

/// A single pending notification from background scheduler activity.
///
/// Notifications are pushed by the scheduler runtime after a scheduled agent
/// turn completes and are surfaced to the user on their next interactive
/// conversation. Unrated notifications are transient — drained after being
/// shown. Rated notifications are preserved as history for agent reflection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// Unique identifier.
    pub id: NotificationId,
    /// The schedule entry name or trigger source that produced this notification.
    pub source: String,
    /// Human-readable summary of the background finding.
    pub content: String,
    /// When the background task completed and this notification was created.
    pub created_at: DateTime<Utc>,
    /// Human feedback rating (None = unrated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
}

impl Notification {
    /// Create a new notification with a fresh ID and the current timestamp.
    pub fn new(source: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: NotificationId::new(),
            source: source.into(),
            content: content.into(),
            created_at: Utc::now(),
            rating: None,
        }
    }
}

/// Encrypted notification queue backed by [`EncryptedStore`].
///
/// Key namespace: `"notif:{NotificationId}"`. Follows the same pattern as
/// `TaskStore` in `aivyx-task` — a thin wrapper around `EncryptedStore` with
/// domain-specific methods.
pub struct NotificationStore {
    store: EncryptedStore,
}

impl NotificationStore {
    /// Open or create a notification store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = EncryptedStore::open(path)?;
        Ok(Self { store })
    }

    /// Push a new notification into the queue.
    pub fn push(&self, notification: &Notification, key: &MasterKey) -> Result<()> {
        let store_key = format!("notif:{}", notification.id);
        let json = serde_json::to_vec(notification)?;
        self.store.put(&store_key, &json, key)
    }

    /// List all pending notifications sorted by creation time (oldest first).
    pub fn list(&self, key: &MasterKey) -> Result<Vec<Notification>> {
        let keys = self.store.list_keys()?;
        let mut notifs: Vec<Notification> = Vec::new();
        for k in keys {
            if k.starts_with("notif:")
                && let Some(bytes) = self.store.get(&k, key)?
            {
                let n: Notification = serde_json::from_slice(&bytes)?;
                notifs.push(n);
            }
        }
        notifs.sort_by_key(|n| n.created_at);
        Ok(notifs)
    }

    /// List and delete all **unrated** pending notifications.
    ///
    /// Rated notifications are preserved as history for the agent feedback
    /// loop. Only unrated notifications are drained (surfaced then cleared).
    pub fn drain(&self, key: &MasterKey) -> Result<Vec<Notification>> {
        let notifs = self.list(key)?;
        let mut drained = Vec::new();
        for n in notifs {
            if n.rating.is_none() {
                if let Err(e) = self.delete(&n.id) {
                    tracing::warn!("failed to delete notification {}: {e}", n.id);
                }
                drained.push(n);
            }
        }
        Ok(drained)
    }

    /// Delete a single notification by ID.
    pub fn delete(&self, id: &NotificationId) -> Result<()> {
        let store_key = format!("notif:{id}");
        self.store.delete(&store_key)
    }

    /// Rate a notification by ID. Returns the updated notification.
    ///
    /// The rating is persisted in-place — the notification is read, updated,
    /// and written back under the same key.
    pub fn rate(
        &self,
        id: &NotificationId,
        rating: Rating,
        key: &MasterKey,
    ) -> Result<Notification> {
        let store_key = format!("notif:{id}");
        let bytes = self.store.get(&store_key, key)?.ok_or_else(|| {
            aivyx_core::AivyxError::Other(format!("notification not found: {id}"))
        })?;
        let mut notif: Notification = serde_json::from_slice(&bytes)?;
        notif.rating = Some(rating);
        let json = serde_json::to_vec(&notif)?;
        self.store.put(&store_key, &json, key)?;
        Ok(notif)
    }

    /// List rated notifications, optionally filtered by source and/or rating.
    ///
    /// Used by the reflection schedule to review past outputs and their
    /// human-assigned quality ratings.
    pub fn list_rated(
        &self,
        key: &MasterKey,
        source_filter: Option<&str>,
        rating_filter: Option<Rating>,
        limit: usize,
    ) -> Result<Vec<Notification>> {
        let all = self.list(key)?;
        let filtered: Vec<Notification> = all
            .into_iter()
            .filter(|n| n.rating.is_some())
            .filter(|n| source_filter.is_none_or(|s| n.source.contains(s)))
            .filter(|n| rating_filter.is_none_or(|r| n.rating == Some(r)))
            .rev() // newest first
            .take(limit)
            .collect();
        Ok(filtered)
    }

    /// Count pending notifications without reading their contents.
    pub fn count(&self) -> Result<usize> {
        let keys = self.store.list_keys()?;
        Ok(keys.iter().filter(|k| k.starts_with("notif:")).count())
    }

    /// Format pending notifications as a system prompt block.
    ///
    /// Returns `None` if there are no pending notifications.
    pub fn format_block(notifications: &[Notification]) -> Option<String> {
        if notifications.is_empty() {
            return None;
        }
        let count = notifications.len();
        let mut block = format!(
            "[BACKGROUND FINDINGS]\n\
             You have {count} pending notification{} from background activity:\n",
            if count == 1 { "" } else { "s" }
        );
        for (i, n) in notifications.iter().enumerate() {
            let ts = n.created_at.format("%Y-%m-%d %H:%M");
            block.push_str(&format!(
                "{}. [{}] {}: {}\n",
                i + 1,
                ts,
                n.source,
                n.content
            ));
        }
        block.push_str(
            "Mention these findings naturally in your response, \
             then they will be cleared.\n\
             [END BACKGROUND FINDINGS]",
        );
        Some(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_new() {
        let n = Notification::new("morning-digest", "CI pipeline is green");
        assert_eq!(n.source, "morning-digest");
        assert_eq!(n.content, "CI pipeline is green");
    }

    #[test]
    fn notification_serde_roundtrip() {
        let n = Notification::new("web-check", "New release found");
        let json = serde_json::to_string(&n).unwrap();
        let parsed: Notification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, n.id);
        assert_eq!(parsed.source, "web-check");
        assert_eq!(parsed.content, "New release found");
    }

    #[test]
    fn notification_store_push_list() {
        let dir = std::env::temp_dir().join(format!("aivyx-notif-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = MasterKey::from_bytes([42u8; 32]);
        let store = NotificationStore::open(dir.join("notifications.db")).unwrap();

        // Initially empty
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.list(&key).unwrap().is_empty());

        // Push two notifications
        let n1 = Notification::new("sched-a", "Result A");
        let n2 = Notification::new("sched-b", "Result B");
        store.push(&n1, &key).unwrap();
        store.push(&n2, &key).unwrap();

        assert_eq!(store.count().unwrap(), 2);
        let listed = store.list(&key).unwrap();
        assert_eq!(listed.len(), 2);
        // Sorted by created_at (oldest first)
        assert_eq!(listed[0].source, "sched-a");
        assert_eq!(listed[1].source, "sched-b");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn notification_store_drain_clears() {
        let dir = std::env::temp_dir().join(format!("aivyx-notif-drain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = MasterKey::from_bytes([42u8; 32]);
        let store = NotificationStore::open(dir.join("notifications.db")).unwrap();

        store.push(&Notification::new("a", "one"), &key).unwrap();
        store.push(&Notification::new("b", "two"), &key).unwrap();
        store.push(&Notification::new("c", "three"), &key).unwrap();

        // Drain returns all and clears
        let drained = store.drain(&key).unwrap();
        assert_eq!(drained.len(), 3);
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.list(&key).unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_block_empty() {
        assert!(NotificationStore::format_block(&[]).is_none());
    }

    #[test]
    fn format_block_with_notifications() {
        let notifs = vec![
            Notification::new("digest", "Good morning"),
            Notification::new("monitor", "CI green"),
        ];
        let block = NotificationStore::format_block(&notifs).unwrap();
        assert!(block.contains("[BACKGROUND FINDINGS]"));
        assert!(block.contains("[END BACKGROUND FINDINGS]"));
        assert!(block.contains("2 pending notifications"));
        assert!(block.contains("digest: Good morning"));
        assert!(block.contains("monitor: CI green"));
    }
}
