//! Encrypted persistence layer for memories, embeddings, knowledge triples,
//! and user profiles.
//!
//! Wraps [`EncryptedStore`] with domain-specific key naming conventions:
//! - `"mem:{MemoryId}"` — serialized [`MemoryEntry`]
//! - `"emb:{MemoryId}"` — serialized embedding vector (`Vec<f32>`)
//! - `"ecache:{sha256_hex}"` — embedding cache (content hash -> `Vec<f32>`)
//! - `"triple:{TripleId}"` — serialized [`KnowledgeTriple`]
//! - `"outcome:{OutcomeId}"` — serialized [`OutcomeRecord`]
//! - `"profile:current"` — serialized [`UserProfile`]
//! - `"profile:v{revision}"` — versioned snapshot before overwrite
//! - `"profile:extract_counter"` — facts accumulated since last extraction

use std::path::Path;

use aivyx_core::{AivyxError, MemoryId, OutcomeId, Result, TripleId};
use aivyx_crypto::{EncryptedStore, MasterKey};

use crate::outcome::{OutcomeFilter, OutcomeRecord};
use crate::profile::UserProfile;
use crate::types::{KnowledgeTriple, MemoryEntry};

/// Encrypted store for memory data, following the `SessionStore` pattern.
pub struct MemoryStore {
    store: EncryptedStore,
}

impl MemoryStore {
    /// Open or create a memory store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = EncryptedStore::open(path)?;
        Ok(Self { store })
    }

    // -----------------------------------------------------------------------
    // Memory entries
    // -----------------------------------------------------------------------

    /// Save a memory entry.
    pub fn save_memory(&self, entry: &MemoryEntry, master_key: &MasterKey) -> Result<()> {
        let key = format!("mem:{}", entry.id);
        let data = serde_json::to_vec(entry).map_err(AivyxError::Serialization)?;
        self.store.put(&key, &data, master_key)
    }

    /// Load a memory entry by ID.
    pub fn load_memory(
        &self,
        id: &MemoryId,
        master_key: &MasterKey,
    ) -> Result<Option<MemoryEntry>> {
        let key = format!("mem:{id}");
        match self.store.get(&key, master_key)? {
            Some(data) => {
                let entry: MemoryEntry = serde_json::from_slice(&data)?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    /// Delete a memory entry by ID.
    pub fn delete_memory(&self, id: &MemoryId) -> Result<()> {
        let key = format!("mem:{id}");
        self.store.delete(&key)
    }

    /// List all memory IDs in the store.
    pub fn list_memories(&self) -> Result<Vec<MemoryId>> {
        let keys = self.store.list_keys()?;
        let mut ids = Vec::new();
        for key in keys {
            if let Some(id_str) = key.strip_prefix("mem:")
                && let Ok(id) = id_str.parse::<MemoryId>()
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    // -----------------------------------------------------------------------
    // Embedding vectors
    // -----------------------------------------------------------------------

    /// Save an embedding vector for a memory.
    pub fn save_embedding(
        &self,
        id: &MemoryId,
        vector: &[f32],
        master_key: &MasterKey,
    ) -> Result<()> {
        let key = format!("emb:{id}");
        let data = serde_json::to_vec(vector).map_err(AivyxError::Serialization)?;
        self.store.put(&key, &data, master_key)
    }

    /// Load an embedding vector for a memory.
    pub fn load_embedding(
        &self,
        id: &MemoryId,
        master_key: &MasterKey,
    ) -> Result<Option<Vec<f32>>> {
        let key = format!("emb:{id}");
        match self.store.get(&key, master_key)? {
            Some(data) => {
                let vector: Vec<f32> = serde_json::from_slice(&data)?;
                Ok(Some(vector))
            }
            None => Ok(None),
        }
    }

    /// Delete an embedding vector for a memory.
    pub fn delete_embedding(&self, id: &MemoryId) -> Result<()> {
        let key = format!("emb:{id}");
        self.store.delete(&key)
    }

    // -----------------------------------------------------------------------
    // Embedding cache (content hash -> vector)
    // -----------------------------------------------------------------------

    /// Cache an embedding vector keyed by content hash.
    pub fn cache_embedding(
        &self,
        content_hash: &str,
        vector: &[f32],
        master_key: &MasterKey,
    ) -> Result<()> {
        let key = format!("ecache:{content_hash}");
        let data = serde_json::to_vec(vector).map_err(AivyxError::Serialization)?;
        self.store.put(&key, &data, master_key)
    }

    /// Look up a cached embedding by content hash.
    pub fn get_cached_embedding(
        &self,
        content_hash: &str,
        master_key: &MasterKey,
    ) -> Result<Option<Vec<f32>>> {
        let key = format!("ecache:{content_hash}");
        match self.store.get(&key, master_key)? {
            Some(data) => {
                let vector: Vec<f32> = serde_json::from_slice(&data)?;
                Ok(Some(vector))
            }
            None => Ok(None),
        }
    }

    // -----------------------------------------------------------------------
    // Binary attachments (images, audio, etc.)
    // -----------------------------------------------------------------------

    /// Save binary attachment data (e.g., an image associated with a memory).
    pub fn save_attachment(
        &self,
        id: &str,
        data: &[u8],
        master_key: &MasterKey,
    ) -> Result<()> {
        let key = format!("attach:{id}");
        self.store.put(&key, data, master_key)
    }

    /// Load binary attachment data by ID.
    pub fn load_attachment(
        &self,
        id: &str,
        master_key: &MasterKey,
    ) -> Result<Option<Vec<u8>>> {
        let key = format!("attach:{id}");
        self.store.get(&key, master_key)
    }

    /// Delete a binary attachment.
    pub fn delete_attachment(&self, id: &str) -> Result<()> {
        let key = format!("attach:{id}");
        self.store.delete(&key)
    }

    // -----------------------------------------------------------------------
    // Knowledge triples
    // -----------------------------------------------------------------------

    /// Save a knowledge triple.
    pub fn save_triple(&self, triple: &KnowledgeTriple, master_key: &MasterKey) -> Result<()> {
        let key = format!("triple:{}", triple.id);
        let data = serde_json::to_vec(triple).map_err(AivyxError::Serialization)?;
        self.store.put(&key, &data, master_key)
    }

    /// Load a knowledge triple by ID.
    pub fn load_triple(
        &self,
        id: &TripleId,
        master_key: &MasterKey,
    ) -> Result<Option<KnowledgeTriple>> {
        let key = format!("triple:{id}");
        match self.store.get(&key, master_key)? {
            Some(data) => {
                let triple: KnowledgeTriple = serde_json::from_slice(&data)?;
                Ok(Some(triple))
            }
            None => Ok(None),
        }
    }

    /// Delete a knowledge triple by ID.
    pub fn delete_triple(&self, id: &TripleId) -> Result<()> {
        let key = format!("triple:{id}");
        self.store.delete(&key)
    }

    /// List all triple IDs in the store.
    pub fn list_triples(&self) -> Result<Vec<TripleId>> {
        let keys = self.store.list_keys()?;
        let mut ids = Vec::new();
        for key in keys {
            if let Some(id_str) = key.strip_prefix("triple:")
                && let Ok(id) = id_str.parse::<TripleId>()
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    // -----------------------------------------------------------------------
    // Outcome records
    // -----------------------------------------------------------------------

    /// Save an outcome record.
    pub fn save_outcome(&self, record: &OutcomeRecord, master_key: &MasterKey) -> Result<()> {
        let key = format!("outcome:{}", record.id);
        let data = serde_json::to_vec(record).map_err(AivyxError::Serialization)?;
        self.store.put(&key, &data, master_key)
    }

    /// Load an outcome record by ID.
    pub fn load_outcome(
        &self,
        id: &OutcomeId,
        master_key: &MasterKey,
    ) -> Result<Option<OutcomeRecord>> {
        let key = format!("outcome:{id}");
        match self.store.get(&key, master_key)? {
            Some(data) => {
                let record: OutcomeRecord = serde_json::from_slice(&data)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Delete an outcome record by ID.
    pub fn delete_outcome(&self, id: &OutcomeId) -> Result<()> {
        let key = format!("outcome:{id}");
        self.store.delete(&key)
    }

    /// List all outcome IDs in the store.
    pub fn list_outcomes(&self) -> Result<Vec<OutcomeId>> {
        let keys = self.store.list_keys()?;
        let mut ids = Vec::new();
        for key in keys {
            if let Some(id_str) = key.strip_prefix("outcome:")
                && let Ok(id) = id_str.parse::<OutcomeId>()
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Query outcomes with optional filters.
    ///
    /// Loads all outcomes and applies the filter criteria: source type name,
    /// success/failure, agent name, and result limit.
    pub fn query_outcomes(
        &self,
        filter: &OutcomeFilter,
        master_key: &MasterKey,
    ) -> Result<Vec<OutcomeRecord>> {
        let ids = self.list_outcomes()?;
        let mut results = Vec::new();

        for id in ids {
            if let Some(record) = self.load_outcome(&id, master_key)? {
                // Source type filter
                if let Some(ref source_type) = filter.source_type {
                    let actual = match &record.source {
                        crate::outcome::OutcomeSource::MissionStep { .. } => "MissionStep",
                        crate::outcome::OutcomeSource::ToolCall { .. } => "ToolCall",
                        crate::outcome::OutcomeSource::Delegation { .. } => "Delegation",
                        crate::outcome::OutcomeSource::SpecialistSuggestion { .. } => {
                            "SpecialistSuggestion"
                        }
                    };
                    if actual != source_type {
                        continue;
                    }
                }

                // Success filter
                if let Some(success) = filter.success
                    && record.success != success
                {
                    continue;
                }

                // Agent name filter
                if let Some(ref agent_name) = filter.agent_name
                    && record.agent_name != *agent_name
                {
                    continue;
                }

                results.push(record);

                // Limit
                if let Some(limit) = filter.limit
                    && results.len() >= limit
                {
                    break;
                }
            }
        }

        Ok(results)
    }

    // -----------------------------------------------------------------------
    // User profile
    // -----------------------------------------------------------------------

    /// Save the current user profile.
    pub fn save_profile(&self, profile: &UserProfile, master_key: &MasterKey) -> Result<()> {
        let data = serde_json::to_vec(profile).map_err(AivyxError::Serialization)?;
        self.store.put("profile:current", &data, master_key)
    }

    /// Load the current user profile. Returns `None` if no profile exists yet.
    pub fn load_profile(&self, master_key: &MasterKey) -> Result<Option<UserProfile>> {
        match self.store.get("profile:current", master_key)? {
            Some(data) => {
                let profile: UserProfile = serde_json::from_slice(&data)?;
                Ok(Some(profile))
            }
            None => Ok(None),
        }
    }

    /// Save a versioned snapshot of the profile before overwriting.
    pub fn save_profile_snapshot(
        &self,
        profile: &UserProfile,
        revision: u64,
        master_key: &MasterKey,
    ) -> Result<()> {
        let key = format!("profile:v{revision}");
        let data = serde_json::to_vec(profile).map_err(AivyxError::Serialization)?;
        self.store.put(&key, &data, master_key)
    }

    /// Load the extraction counter (facts accumulated since last profile
    /// extraction). Returns `0` if no counter exists.
    pub fn load_extraction_counter(&self, master_key: &MasterKey) -> Result<u64> {
        match self.store.get("profile:extract_counter", master_key)? {
            Some(data) => {
                let count: u64 = serde_json::from_slice(&data)?;
                Ok(count)
            }
            None => Ok(0),
        }
    }

    /// Save the extraction counter.
    pub fn save_extraction_counter(&self, count: u64, master_key: &MasterKey) -> Result<()> {
        let data = serde_json::to_vec(&count).map_err(AivyxError::Serialization)?;
        self.store.put("profile:extract_counter", &data, master_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{OutcomeRecord, OutcomeSource};
    use crate::types::{KnowledgeTriple, MemoryEntry, MemoryKind};
    use aivyx_core::TaskId;
    use aivyx_crypto::MasterKey;

    fn setup() -> (MemoryStore, MasterKey, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("aivyx-memory-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db");
        let store = MemoryStore::open(&db_path).unwrap();
        let master_key = MasterKey::generate();
        (store, master_key, dir)
    }

    #[test]
    fn save_load_memory() {
        let (store, key, dir) = setup();
        let entry = MemoryEntry::new(
            "Rust is memory safe".into(),
            MemoryKind::Fact,
            None,
            vec!["rust".into()],
        );
        let id = entry.id;

        store.save_memory(&entry, &key).unwrap();
        let loaded = store.load_memory(&id, &key).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.content, "Rust is memory safe");
        assert_eq!(loaded.kind, MemoryKind::Fact);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_memory() {
        let (store, key, dir) = setup();
        let entry = MemoryEntry::new("temp".into(), MemoryKind::Fact, None, vec![]);
        let id = entry.id;

        store.save_memory(&entry, &key).unwrap();
        assert!(store.load_memory(&id, &key).unwrap().is_some());

        store.delete_memory(&id).unwrap();
        assert!(store.load_memory(&id, &key).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_memories() {
        let (store, key, dir) = setup();
        let e1 = MemoryEntry::new("one".into(), MemoryKind::Fact, None, vec![]);
        let e2 = MemoryEntry::new("two".into(), MemoryKind::Preference, None, vec![]);

        store.save_memory(&e1, &key).unwrap();
        store.save_memory(&e2, &key).unwrap();

        let ids = store.list_memories().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&e1.id));
        assert!(ids.contains(&e2.id));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_nonexistent_memory_returns_none() {
        let (store, key, dir) = setup();
        let result = store.load_memory(&MemoryId::new(), &key).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_load_embedding() {
        let (store, key, dir) = setup();
        let id = MemoryId::new();
        let vector = vec![0.1_f32, 0.2, 0.3];

        store.save_embedding(&id, &vector, &key).unwrap();
        let loaded = store.load_embedding(&id, &key).unwrap().unwrap();
        assert_eq!(loaded.len(), 3);
        assert!((loaded[0] - 0.1).abs() < f32::EPSILON);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_embedding() {
        let (store, key, dir) = setup();
        let id = MemoryId::new();
        store.save_embedding(&id, &[1.0, 2.0], &key).unwrap();
        store.delete_embedding(&id).unwrap();
        assert!(store.load_embedding(&id, &key).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_nonexistent_embedding_returns_none() {
        let (store, key, dir) = setup();
        let result = store.load_embedding(&MemoryId::new(), &key).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_embedding_hit() {
        let (store, key, dir) = setup();
        let hash = "abc123def456";
        let vector = vec![0.5_f32, 0.6, 0.7];

        store.cache_embedding(hash, &vector, &key).unwrap();
        let cached = store.get_cached_embedding(hash, &key).unwrap().unwrap();
        assert_eq!(cached, vector);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_embedding_miss() {
        let (store, key, dir) = setup();
        let result = store.get_cached_embedding("nonexistent", &key).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_load_triple() {
        let (store, key, dir) = setup();
        let triple = KnowledgeTriple::new(
            "Rust".into(),
            "is_a".into(),
            "language".into(),
            None,
            0.9,
            "user".into(),
        );
        let id = triple.id;

        store.save_triple(&triple, &key).unwrap();
        let loaded = store.load_triple(&id, &key).unwrap().unwrap();
        assert_eq!(loaded.subject, "Rust");
        assert_eq!(loaded.predicate, "is_a");
        assert_eq!(loaded.object, "language");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_triple() {
        let (store, key, dir) = setup();
        let triple =
            KnowledgeTriple::new("A".into(), "B".into(), "C".into(), None, 1.0, "test".into());
        let id = triple.id;

        store.save_triple(&triple, &key).unwrap();
        store.delete_triple(&id).unwrap();
        assert!(store.load_triple(&id, &key).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_triples() {
        let (store, key, dir) = setup();
        let t1 = KnowledgeTriple::new(
            "A".into(),
            "rel".into(),
            "B".into(),
            None,
            1.0,
            "test".into(),
        );
        let t2 = KnowledgeTriple::new(
            "C".into(),
            "rel".into(),
            "D".into(),
            None,
            0.8,
            "test".into(),
        );

        store.save_triple(&t1, &key).unwrap();
        store.save_triple(&t2, &key).unwrap();

        let ids = store.list_triples().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&t1.id));
        assert!(ids.contains(&t2.id));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_load_profile_roundtrip() {
        let (store, key, dir) = setup();
        let mut profile = crate::profile::UserProfile::new();
        profile.name = Some("Julian".into());
        profile.tech_stack = vec!["Rust".into()];
        profile.revision = 3;

        store.save_profile(&profile, &key).unwrap();
        let loaded = store.load_profile(&key).unwrap().unwrap();
        assert_eq!(loaded.name.as_deref(), Some("Julian"));
        assert_eq!(loaded.tech_stack, vec!["Rust"]);
        assert_eq!(loaded.revision, 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_profile_returns_none_when_missing() {
        let (store, key, dir) = setup();
        let result = store.load_profile(&key).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_profile_snapshot() {
        let (store, key, dir) = setup();
        let mut profile = crate::profile::UserProfile::new();
        profile.name = Some("Julian".into());
        profile.revision = 5;

        store.save_profile_snapshot(&profile, 5, &key).unwrap();

        // Snapshot is at "profile:v5" — can't load via load_profile (that reads "profile:current")
        // but we can verify it doesn't collide with the current profile
        assert!(store.load_profile(&key).unwrap().is_none());

        // Save a different current profile
        let mut current = crate::profile::UserProfile::new();
        current.name = Some("Updated".into());
        store.save_profile(&current, &key).unwrap();

        let loaded = store.load_profile(&key).unwrap().unwrap();
        assert_eq!(loaded.name.as_deref(), Some("Updated"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extraction_counter_roundtrip() {
        let (store, key, dir) = setup();

        // Default is 0
        assert_eq!(store.load_extraction_counter(&key).unwrap(), 0);

        // Set to 15
        store.save_extraction_counter(15, &key).unwrap();
        assert_eq!(store.load_extraction_counter(&key).unwrap(), 15);

        // Reset to 0
        store.save_extraction_counter(0, &key).unwrap();
        assert_eq!(store.load_extraction_counter(&key).unwrap(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extraction_counter_default_zero() {
        let (store, key, dir) = setup();
        let count = store.load_extraction_counter(&key).unwrap();
        assert_eq!(count, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_load_outcome() {
        let (store, key, dir) = setup();
        let record = OutcomeRecord::new(
            OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            true,
            "Command succeeded".into(),
            250,
            "test-agent".into(),
            "run build".into(),
        );
        let id = record.id;

        store.save_outcome(&record, &key).unwrap();
        let loaded = store.load_outcome(&id, &key).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert!(loaded.success);
        assert_eq!(loaded.agent_name, "test-agent");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_outcome() {
        let (store, key, dir) = setup();
        let record = OutcomeRecord::new(
            OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            true,
            "ok".into(),
            100,
            "agent".into(),
            "goal".into(),
        );
        let id = record.id;

        store.save_outcome(&record, &key).unwrap();
        assert!(store.load_outcome(&id, &key).unwrap().is_some());

        store.delete_outcome(&id).unwrap();
        assert!(store.load_outcome(&id, &key).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_outcomes() {
        let (store, key, dir) = setup();
        let r1 = OutcomeRecord::new(
            OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            true,
            "ok".into(),
            100,
            "agent".into(),
            "goal".into(),
        );
        let r2 = OutcomeRecord::new(
            OutcomeSource::Delegation {
                specialist: "coder".into(),
                task: "write code".into(),
            },
            false,
            "failed".into(),
            500,
            "lead".into(),
            "deploy".into(),
        );

        store.save_outcome(&r1, &key).unwrap();
        store.save_outcome(&r2, &key).unwrap();

        let ids = store.list_outcomes().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&r1.id));
        assert!(ids.contains(&r2.id));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_outcomes_by_source_type() {
        let (store, key, dir) = setup();

        let tool_outcome = OutcomeRecord::new(
            OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            true,
            "ok".into(),
            100,
            "agent".into(),
            "goal".into(),
        );
        let delegation_outcome = OutcomeRecord::new(
            OutcomeSource::Delegation {
                specialist: "coder".into(),
                task: "code".into(),
            },
            true,
            "ok".into(),
            200,
            "agent".into(),
            "goal".into(),
        );

        store.save_outcome(&tool_outcome, &key).unwrap();
        store.save_outcome(&delegation_outcome, &key).unwrap();

        let filter = OutcomeFilter {
            source_type: Some("ToolCall".into()),
            ..Default::default()
        };
        let results = store.query_outcomes(&filter, &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, tool_outcome.id);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_outcomes_by_success_and_agent() {
        let (store, key, dir) = setup();

        let r1 = OutcomeRecord::new(
            OutcomeSource::MissionStep {
                task_id: TaskId::new(),
                step_index: 0,
            },
            true,
            "ok".into(),
            100,
            "alpha".into(),
            "goal".into(),
        );
        let r2 = OutcomeRecord::new(
            OutcomeSource::MissionStep {
                task_id: TaskId::new(),
                step_index: 1,
            },
            false,
            "failed".into(),
            200,
            "alpha".into(),
            "goal".into(),
        );
        let r3 = OutcomeRecord::new(
            OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            true,
            "ok".into(),
            50,
            "beta".into(),
            "goal".into(),
        );

        store.save_outcome(&r1, &key).unwrap();
        store.save_outcome(&r2, &key).unwrap();
        store.save_outcome(&r3, &key).unwrap();

        // Filter: successful only
        let filter = OutcomeFilter {
            success: Some(true),
            ..Default::default()
        };
        let results = store.query_outcomes(&filter, &key).unwrap();
        assert_eq!(results.len(), 2);

        // Filter: agent "alpha" + failed
        let filter = OutcomeFilter {
            success: Some(false),
            agent_name: Some("alpha".into()),
            ..Default::default()
        };
        let results = store.query_outcomes(&filter, &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, r2.id);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_outcomes_with_limit() {
        let (store, key, dir) = setup();

        for i in 0..5 {
            let record = OutcomeRecord::new(
                OutcomeSource::ToolCall {
                    tool_name: format!("tool_{i}"),
                },
                true,
                "ok".into(),
                100,
                "agent".into(),
                "goal".into(),
            );
            store.save_outcome(&record, &key).unwrap();
        }

        let filter = OutcomeFilter {
            limit: Some(3),
            ..Default::default()
        };
        let results = store.query_outcomes(&filter, &key).unwrap();
        assert_eq!(results.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_nonexistent_outcome_returns_none() {
        let (store, key, dir) = setup();
        let result = store.load_outcome(&OutcomeId::new(), &key).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
