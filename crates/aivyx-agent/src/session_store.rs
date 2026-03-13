use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use aivyx_core::{AivyxError, Result, SessionId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::ChatMessage;

/// Metadata about a persisted session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: SessionId,
    pub agent_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
}

/// A complete persisted session including metadata and messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    pub metadata: SessionMetadata,
    pub messages: Vec<ChatMessage>,
}

/// Encrypted storage for chat sessions.
pub struct SessionStore {
    store: EncryptedStore,
}

impl SessionStore {
    /// Open or create a session store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = EncryptedStore::open(path)?;
        Ok(Self { store })
    }

    /// Save a session to the store.
    pub fn save(&self, session: &PersistedSession, master_key: &MasterKey) -> Result<()> {
        let key = format!("session:{}", session.metadata.session_id);
        let data = serde_json::to_vec(session).map_err(AivyxError::Serialization)?;
        self.store.put(&key, &data, master_key)
    }

    /// Load a session by ID.
    ///
    /// If `max_age_hours` is non-zero, sessions whose `updated_at` timestamp
    /// is older than the limit are deleted from the store and `None` is returned.
    /// Pass `0` to disable expiry checking.
    pub fn load(
        &self,
        session_id: &SessionId,
        master_key: &MasterKey,
        max_age_hours: u64,
    ) -> Result<Option<PersistedSession>> {
        let key = format!("session:{session_id}");
        match self.store.get(&key, master_key)? {
            Some(data) => {
                let session: PersistedSession = serde_json::from_slice(&data)?;
                if max_age_hours > 0 {
                    let age = chrono::Utc::now() - session.metadata.updated_at;
                    if age.num_hours() as u64 > max_age_hours {
                        // Session expired — delete and return None
                        let _ = self.delete(session_id);
                        return Ok(None);
                    }
                }
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    /// List all session metadata (loads each session to extract metadata).
    pub fn list(&self, master_key: &MasterKey) -> Result<Vec<SessionMetadata>> {
        let keys = self.store.list_keys()?;
        let mut sessions = Vec::new();

        for key in keys {
            if let Some(id_str) = key.strip_prefix("session:")
                && let Ok(Some(data)) = self.store.get(&key, master_key)
                && let Ok(session) = serde_json::from_slice::<PersistedSession>(&data)
                && session.metadata.session_id.to_string() == id_str
            {
                sessions.push(session.metadata);
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    /// Lightweight probe to verify the store is accessible.
    ///
    /// Used by the `/health` endpoint to detect redb lock/corruption issues.
    pub fn health_probe(&self) -> Result<()> {
        self.store.health_probe()
    }

    /// Delete a session by ID.
    pub fn delete(&self, session_id: &SessionId) -> Result<()> {
        let key = format!("session:{session_id}");
        self.store.delete(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_crypto::MasterKey;

    fn setup() -> (SessionStore, MasterKey, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("aivyx-session-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("sessions.db");
        let store = SessionStore::open(&db_path).unwrap();
        let master_key = MasterKey::generate();
        (store, master_key, dir)
    }

    fn make_session(agent_name: &str, message_count: usize) -> PersistedSession {
        let mut messages = Vec::new();
        for i in 0..message_count {
            if i % 2 == 0 {
                messages.push(ChatMessage::user(format!("msg {i}")));
            } else {
                messages.push(ChatMessage::assistant(format!("reply {i}")));
            }
        }

        PersistedSession {
            metadata: SessionMetadata {
                session_id: SessionId::new(),
                agent_name: agent_name.to_string(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                message_count,
            },
            messages,
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let (store, key, dir) = setup();
        let session = make_session("test-agent", 4);
        let id = session.metadata.session_id;

        store.save(&session, &key).unwrap();
        let loaded = store.load(&id, &key, 0).unwrap().unwrap();

        assert_eq!(loaded.metadata.session_id, id);
        assert_eq!(loaded.metadata.agent_name, "test-agent");
        assert_eq!(loaded.messages.len(), 4);
        assert_eq!(loaded.messages[0].content, "msg 0");
        assert_eq!(loaded.messages[1].content, "reply 1");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let (store, key, dir) = setup();
        let result = store.load(&SessionId::new(), &key, 0).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_expired_session_returns_none() {
        let (store, key, dir) = setup();
        let session = make_session("old-agent", 2);
        let id = session.metadata.session_id;

        store.save(&session, &key).unwrap();

        // max_age_hours = 0 means no expiry — still loads
        assert!(store.load(&id, &key, 0).unwrap().is_some());

        // max_age_hours = 1 — session updated_at is ~now, should not expire
        assert!(store.load(&id, &key, 1).unwrap().is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_metadata() {
        let (store, key, dir) = setup();
        let s1 = make_session("agent-a", 2);
        let s2 = make_session("agent-b", 6);

        store.save(&s1, &key).unwrap();
        store.save(&s2, &key).unwrap();

        let list = store.list(&key).unwrap();
        assert_eq!(list.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_session() {
        let (store, key, dir) = setup();
        let session = make_session("test", 2);
        let id = session.metadata.session_id;

        store.save(&session, &key).unwrap();
        assert!(store.load(&id, &key, 0).unwrap().is_some());

        store.delete(&id).unwrap();
        assert!(store.load(&id, &key, 0).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
