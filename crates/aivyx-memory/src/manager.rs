//! High-level memory manager that coordinates embedding, caching, storage,
//! search, and context formatting.

use std::sync::Arc;

use tracing::debug;

use aivyx_core::{AgentId, MemoryId, OutcomeId, PatternId, Result, TripleId};
use aivyx_crypto::MasterKey;
use aivyx_llm::EmbeddingProvider;

use crate::graph::KnowledgeGraph;
use crate::outcome::{OutcomeFilter, OutcomeRecord};
use crate::search::{SearchResult, VectorIndex, content_hash};
use crate::store::MemoryStore;
use crate::types::{KnowledgeTriple, MemoryEntry, MemoryKind};

/// Statistics about the memory subsystem.
#[derive(Debug, Clone)]
pub struct MemoryStats {
    /// Total number of stored memories.
    pub total_memories: usize,
    /// Total number of stored knowledge triples.
    pub total_triples: usize,
    /// Number of vectors in the search index.
    pub index_size: usize,
}

/// Orchestrates embedding, caching, storage, search, and formatting for agent
/// memories and knowledge triples.
pub struct MemoryManager {
    pub(crate) store: MemoryStore,
    pub(crate) index: VectorIndex,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    pub(crate) master_key: MasterKey,
    /// Maximum number of memories to keep. `0` means unlimited.
    max_memories: usize,
    /// In-memory knowledge graph for traversal and path finding.
    graph: Option<KnowledgeGraph>,
}

impl MemoryManager {
    /// Create a new `MemoryManager`, building the vector index from existing
    /// embeddings in the store.
    ///
    /// `max_memories` sets the pruning limit — `0` means unlimited.
    pub fn new(
        store: MemoryStore,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        master_key: MasterKey,
        max_memories: usize,
    ) -> Result<Self> {
        let index = VectorIndex::build(&store, &master_key)?;
        let graph = KnowledgeGraph::build(&store, &master_key).ok();
        debug!(
            "MemoryManager initialized with {} indexed vectors (limit: {})",
            index.len(),
            if max_memories == 0 {
                "unlimited".to_string()
            } else {
                max_memories.to_string()
            }
        );
        Ok(Self {
            store,
            index,
            embedding_provider,
            master_key,
            max_memories,
            graph,
        })
    }

    /// Check if content similar to `vector` already exists in the index.
    ///
    /// Returns the existing [`MemoryId`] if any stored embedding has cosine
    /// similarity ≥ `threshold` (typically 0.95 for near-duplicate detection).
    pub fn find_near_duplicate(&self, vector: &[f32], threshold: f32) -> Option<MemoryId> {
        let results = self.index.search(vector, 1);
        results.first().and_then(|r| {
            if r.similarity >= threshold {
                Some(r.memory_id)
            } else {
                None
            }
        })
    }

    /// Store a new memory: embed it (with cache), deduplicate, persist
    /// entry + embedding, and update the search index.
    ///
    /// If a near-duplicate already exists (cosine similarity ≥ 0.95), the new
    /// memory is skipped and the existing [`MemoryId`] is returned instead.
    pub async fn remember(
        &mut self,
        content: String,
        kind: MemoryKind,
        agent_scope: Option<AgentId>,
        tags: Vec<String>,
    ) -> Result<MemoryId> {
        let entry = MemoryEntry::new(content.clone(), kind, agent_scope, tags);
        let id = entry.id;

        // Embed with cache check
        let vector = self.embed_with_cache(&content).await?;

        // Content dedup: skip if near-duplicate exists (cosine > 0.95)
        if let Some(existing_id) = self.find_near_duplicate(&vector, 0.95) {
            debug!("Skipping near-duplicate of memory {existing_id}");
            return Ok(existing_id);
        }

        // Persist
        self.store.save_memory(&entry, &self.master_key)?;
        self.store.save_embedding(&id, &vector, &self.master_key)?;

        // Update index
        self.index.upsert(id, vector);

        debug!("Stored memory {id}");

        // Prune if over limit
        self.prune_to_limit()?;

        Ok(id)
    }

    /// Retrieve the most relevant memories for a query, filtered by agent scope
    /// and optional required tags.
    ///
    /// If `required_tags` is non-empty, only memories that contain **all** of
    /// the specified tags are returned. Pass `&[]` for unfiltered recall.
    pub async fn recall(
        &mut self,
        query: &str,
        top_k: usize,
        agent_id: Option<AgentId>,
        required_tags: &[String],
    ) -> Result<Vec<MemoryEntry>> {
        // Over-fetch more aggressively when tag filtering is active
        let fetch_factor = if required_tags.is_empty() { 2 } else { 4 };
        let query_vec = self.embed_with_cache(query).await?;
        let results = self.index.search(&query_vec, top_k * fetch_factor);

        let mut entries = Vec::new();
        for SearchResult { memory_id, .. } in results {
            if let Some(mut entry) = self.store.load_memory(&memory_id, &self.master_key)? {
                // Scope filter: include if global OR matches the requesting agent
                let visible = entry.agent_scope.is_none() || entry.agent_scope == agent_id;

                if !visible {
                    continue;
                }

                // Tag filter: if required_tags is non-empty, the entry must
                // contain ALL of them.
                if !required_tags.is_empty()
                    && !required_tags.iter().all(|t| entry.tags.contains(t))
                {
                    continue;
                }

                entry.record_access();
                self.store.save_memory(&entry, &self.master_key)?;
                entries.push(entry);
                if entries.len() >= top_k {
                    break;
                }
            }
        }

        debug!(
            "Recalled {} memories for query (top_k={top_k})",
            entries.len()
        );
        Ok(entries)
    }

    /// Access the in-memory knowledge graph, if available.
    pub fn graph(&self) -> Option<&KnowledgeGraph> {
        self.graph.as_ref()
    }

    /// Add a knowledge triple.
    pub fn add_triple(
        &mut self,
        subject: String,
        predicate: String,
        object: String,
        agent_scope: Option<AgentId>,
        confidence: f32,
        source: String,
    ) -> Result<TripleId> {
        let triple =
            KnowledgeTriple::new(subject, predicate, object, agent_scope, confidence, source);
        let id = triple.id;
        self.store.save_triple(&triple, &self.master_key)?;
        if let Some(ref mut graph) = self.graph {
            graph.upsert_triple(&triple);
        }
        debug!("Stored triple {id}");
        Ok(id)
    }

    /// Query knowledge triples with optional filters.
    pub fn query_triples(
        &self,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        agent_id: Option<AgentId>,
    ) -> Result<Vec<KnowledgeTriple>> {
        let ids = self.store.list_triples()?;
        let mut results = Vec::new();

        for id in ids {
            if let Some(triple) = self.store.load_triple(&id, &self.master_key)? {
                // Scope filter
                let visible = triple.agent_scope.is_none() || triple.agent_scope == agent_id;

                if !visible {
                    continue;
                }

                // Field filters
                if let Some(s) = subject
                    && triple.subject != s
                {
                    continue;
                }
                if let Some(p) = predicate
                    && triple.predicate != p
                {
                    continue;
                }
                if let Some(o) = object
                    && triple.object != o
                {
                    continue;
                }

                results.push(triple);
            }
        }

        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Triple confidence evolution
    // -----------------------------------------------------------------------

    /// Reinforce a triple's confidence by the given boost (clamped to 1.0).
    ///
    /// Also updates the in-memory knowledge graph so queries reflect the new
    /// confidence immediately.
    pub fn reinforce_triple(&mut self, id: &TripleId, boost: f32) -> Result<KnowledgeTriple> {
        let triple = self.store.reinforce_triple(id, boost, &self.master_key)?;
        if let Some(ref mut graph) = self.graph {
            graph.upsert_triple(&triple);
        }
        debug!(
            "Reinforced triple {id}: confidence={:.2}, count={}",
            triple.confidence, triple.reinforce_count
        );
        Ok(triple)
    }

    /// Add a new triple, or reinforce an existing one if a triple with the
    /// same subject-predicate-object already exists.
    ///
    /// Returns the triple ID and whether it was newly created.
    #[allow(clippy::too_many_arguments)]
    pub fn add_or_reinforce_triple(
        &mut self,
        subject: String,
        predicate: String,
        object: String,
        agent_scope: Option<AgentId>,
        confidence: f32,
        source: String,
        reinforce_boost: f32,
    ) -> Result<(TripleId, bool)> {
        if let Some(existing_id) =
            self.store
                .find_triple(&subject, &predicate, &object, &self.master_key)?
        {
            self.reinforce_triple(&existing_id, reinforce_boost)?;
            Ok((existing_id, false))
        } else {
            let id =
                self.add_triple(subject, predicate, object, agent_scope, confidence, source)?;
            Ok((id, true))
        }
    }

    /// Apply multiplicative decay to all triple confidences and prune those
    /// below the minimum threshold.
    ///
    /// Rebuilds the in-memory knowledge graph when any triples are pruned.
    pub fn decay_triples(&mut self, factor: f32, min_confidence: f32) -> Result<(usize, usize)> {
        let (decayed, pruned) =
            self.store
                .decay_triples(factor, min_confidence, &self.master_key)?;
        if pruned > 0 {
            // Rebuild graph to drop pruned triples
            self.graph = Some(KnowledgeGraph::build(&self.store, &self.master_key)?);
        } else if decayed > 0 {
            // Confidence values changed — rebuild so graph edges are accurate
            self.graph = Some(KnowledgeGraph::build(&self.store, &self.master_key)?);
        }
        debug!(
            "Triple decay: {decayed} decayed, {pruned} pruned (factor={factor}, min={min_confidence})"
        );
        Ok((decayed, pruned))
    }

    // -----------------------------------------------------------------------
    // Outcome tracking
    // -----------------------------------------------------------------------

    /// Record an outcome from an agent operation.
    pub fn record_outcome(&self, record: &OutcomeRecord) -> Result<OutcomeId> {
        self.store.save_outcome(record, &self.master_key)?;
        debug!(
            "Recorded outcome {} (success={})",
            record.id, record.success
        );
        Ok(record.id)
    }

    /// Query outcomes matching the given filter criteria.
    pub fn query_outcomes(&self, filter: &OutcomeFilter) -> Result<Vec<OutcomeRecord>> {
        self.store.query_outcomes(filter, &self.master_key)
    }

    /// Rate an outcome by ID with an optional feedback comment.
    ///
    /// Updates the outcome in-place and returns the modified record. The rating
    /// feeds into [`LearnedWeights`] so human feedback influences future
    /// specialist selection.
    pub fn rate_outcome(
        &self,
        id: &OutcomeId,
        rating: crate::notification::Rating,
        feedback: Option<String>,
    ) -> Result<OutcomeRecord> {
        let record = self
            .store
            .rate_outcome(id, rating, feedback, &self.master_key)?;
        debug!(
            "Rated outcome {} as {:?}{}",
            id,
            record.human_rating,
            record
                .human_feedback
                .as_ref()
                .map(|f| format!(" — {f}"))
                .unwrap_or_default()
        );
        Ok(record)
    }

    /// Load a single outcome by ID.
    pub fn get_outcome(&self, id: &OutcomeId) -> Result<Option<OutcomeRecord>> {
        self.store.load_outcome(id, &self.master_key)
    }

    // -----------------------------------------------------------------------
    // Workflow pattern mining
    // -----------------------------------------------------------------------

    /// Mine workflow patterns from stored outcomes.
    ///
    /// Loads all outcomes, runs the pattern miner, and persists discovered
    /// patterns. Existing patterns with the same sequence key are updated
    /// rather than duplicated.
    pub fn mine_patterns(
        &self,
        config: &crate::pattern::MiningConfig,
    ) -> Result<Vec<crate::pattern::WorkflowPattern>> {
        let outcomes = self.query_outcomes(&OutcomeFilter {
            limit: Some(1000),
            ..Default::default()
        })?;

        let discovered = crate::pattern::mine_patterns(&outcomes, config);

        let mut stored = Vec::new();
        for mut pattern in discovered {
            // Check for existing pattern with same sequence key
            if let Some(existing_id) = self
                .store
                .find_pattern_by_key(&pattern.sequence_key, &self.master_key)?
            {
                // Update existing pattern's stats
                if let Some(mut existing) =
                    self.store.load_pattern(&existing_id, &self.master_key)?
                {
                    existing.success_rate = pattern.success_rate;
                    existing.occurrence_count = pattern.occurrence_count;
                    existing.success_count = pattern.success_count;
                    existing.avg_duration_ms = pattern.avg_duration_ms;
                    existing.goal_keywords = pattern.goal_keywords;
                    existing.agent_roles = pattern.agent_roles;
                    existing.updated_at = chrono::Utc::now();
                    self.store.save_pattern(&existing, &self.master_key)?;
                    stored.push(existing);
                    continue;
                }
            }
            // New pattern — assign fresh ID and save
            pattern.id = PatternId::new();
            self.store.save_pattern(&pattern, &self.master_key)?;
            stored.push(pattern);
        }

        debug!("Pattern mining: {} patterns stored/updated", stored.len());
        Ok(stored)
    }

    /// Query stored workflow patterns with optional filters.
    pub fn query_patterns(
        &self,
        filter: &crate::pattern::PatternFilter,
    ) -> Result<Vec<crate::pattern::WorkflowPattern>> {
        let ids = self.store.list_patterns()?;
        let mut results = Vec::new();

        for id in ids {
            if let Some(pattern) = self.store.load_pattern(&id, &self.master_key)? {
                if let Some(ref tool) = filter.contains_tool
                    && !pattern.tool_sequence.contains(tool)
                {
                    continue;
                }
                if let Some(min_rate) = filter.min_success_rate
                    && pattern.success_rate < min_rate
                {
                    continue;
                }
                if let Some(min_occ) = filter.min_occurrences
                    && pattern.occurrence_count < min_occ
                {
                    continue;
                }
                results.push(pattern);
            }
        }

        // Sort by occurrence count descending
        results.sort_by(|a, b| b.occurrence_count.cmp(&a.occurrence_count));

        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Load a single pattern by ID.
    pub fn get_pattern(&self, id: &PatternId) -> Result<Option<crate::pattern::WorkflowPattern>> {
        self.store.load_pattern(id, &self.master_key)
    }

    // -----------------------------------------------------------------------
    // Memory consolidation
    // -----------------------------------------------------------------------

    /// Consolidate memories by merging similar entries, pruning stale ones,
    /// and strengthening frequently accessed ones.
    pub async fn consolidate(
        &mut self,
        provider: &dyn aivyx_llm::LlmProvider,
        config: &crate::consolidation::ConsolidationConfig,
    ) -> Result<crate::consolidation::ConsolidationReport> {
        crate::consolidation::consolidate(self, provider, config).await
    }

    /// Delete a memory and its associated embedding.
    pub fn forget(&mut self, memory_id: &MemoryId) -> Result<()> {
        self.store.delete_memory(memory_id)?;
        self.store.delete_embedding(memory_id)?;
        self.index.remove(memory_id);
        debug!("Forgot memory {memory_id}");
        Ok(())
    }

    /// Format retrieved memories and triples as a context block for system
    /// prompt augmentation.
    pub fn format_context(memories: &[MemoryEntry], triples: &[KnowledgeTriple]) -> String {
        let mut out = String::from("[MEMORY CONTEXT]\n");

        if !memories.is_empty() {
            out.push_str("Relevant memories:\n");
            for (i, m) in memories.iter().enumerate() {
                out.push_str(&format!(
                    "{}. [{}] {}\n",
                    i + 1,
                    format_kind(&m.kind),
                    m.content
                ));
            }
        }

        if !triples.is_empty() {
            if !memories.is_empty() {
                out.push('\n');
            }
            out.push_str("Known facts:\n");
            for t in triples {
                out.push_str(&format!(
                    "- {} {} {} (confidence: {:.0}%)\n",
                    t.subject,
                    t.predicate,
                    t.object,
                    t.confidence * 100.0
                ));
            }
        }

        out.push_str("[END MEMORY CONTEXT]");
        out
    }

    /// List all stored memory IDs.
    pub fn list_memories(&self) -> Result<Vec<MemoryId>> {
        self.store.list_memories()
    }

    /// Load a memory entry by ID.
    pub fn load_memory(&self, id: &MemoryId) -> Result<Option<MemoryEntry>> {
        self.store.load_memory(id, &self.master_key)
    }

    /// Get memory subsystem statistics.
    pub fn stats(&self) -> Result<MemoryStats> {
        Ok(MemoryStats {
            total_memories: self.store.list_memories()?.len(),
            total_triples: self.store.list_triples()?.len(),
            index_size: self.index.len(),
        })
    }

    /// Store a raw memory entry and its pre-computed embedding vector.
    ///
    /// This bypasses the embedding provider and deduplication check, persisting
    /// the entry and vector directly. Useful for bulk imports and testing.
    pub fn store_raw(&mut self, entry: &MemoryEntry, vector: &[f32]) -> Result<()> {
        self.store.save_memory(entry, &self.master_key)?;
        self.store
            .save_embedding(&entry.id, vector, &self.master_key)?;
        self.index.upsert(entry.id, vector.to_vec());
        Ok(())
    }

    /// Enforce the memory limit by pruning the least-accessed memories.
    ///
    /// When `max_memories` is `0` (unlimited), this is a no-op. Otherwise,
    /// memories are sorted by `access_count` ascending then `created_at`
    /// ascending, and the least-used ones are removed until the count is
    /// within the limit.
    ///
    /// This is called automatically at the end of [`Self::remember()`], but can
    /// also be called manually after bulk imports via [`Self::store_raw()`].
    pub fn prune_to_limit(&mut self) -> Result<()> {
        if self.max_memories == 0 {
            return Ok(());
        }
        let ids = self.store.list_memories()?;
        if ids.len() <= self.max_memories {
            return Ok(());
        }
        // Load all entries, sort by access_count asc, then created_at asc
        let mut entries: Vec<MemoryEntry> = ids
            .iter()
            .filter_map(|id| self.store.load_memory(id, &self.master_key).ok().flatten())
            .collect();
        entries.sort_by(|a, b| {
            a.access_count
                .cmp(&b.access_count)
                .then(a.created_at.cmp(&b.created_at))
        });
        let to_remove = entries.len() - self.max_memories;
        for entry in entries.iter().take(to_remove) {
            self.forget(&entry.id)?;
        }
        debug!("Pruned {to_remove} memories (limit: {})", self.max_memories);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // User profile
    // -----------------------------------------------------------------------

    /// Get the user profile. Returns an empty default if none exists yet.
    pub fn get_profile(&self) -> Result<crate::profile::UserProfile> {
        match self.store.load_profile(&self.master_key)? {
            Some(profile) => Ok(profile),
            None => Ok(crate::profile::UserProfile::new()),
        }
    }

    /// Update the user profile.
    ///
    /// Saves a versioned snapshot of the previous profile before overwriting,
    /// increments the revision, and updates the timestamp.
    pub fn update_profile(&self, mut profile: crate::profile::UserProfile) -> Result<()> {
        // Snapshot the current profile before overwriting
        if let Some(current) = self.store.load_profile(&self.master_key)? {
            self.store
                .save_profile_snapshot(&current, current.revision, &self.master_key)?;
        }

        profile.updated_at = chrono::Utc::now();
        profile.revision = profile.revision.saturating_add(1);
        self.store.save_profile(&profile, &self.master_key)?;
        debug!("Updated user profile (revision {})", profile.revision);
        Ok(())
    }

    /// Increment the extraction counter and return the new value.
    ///
    /// Used to track how many facts have accumulated since the last profile
    /// extraction. When the counter exceeds the configured threshold, a
    /// profile extraction should be triggered.
    pub fn increment_extraction_counter(&self) -> Result<u64> {
        let current = self.store.load_extraction_counter(&self.master_key)?;
        let new_count = current.saturating_add(1);
        self.store
            .save_extraction_counter(new_count, &self.master_key)?;
        Ok(new_count)
    }

    /// Reset the extraction counter to zero (after a profile extraction).
    pub fn reset_extraction_counter(&self) -> Result<()> {
        self.store.save_extraction_counter(0, &self.master_key)
    }

    /// Extract a structured profile from accumulated Fact and Preference memories
    /// using the LLM.
    ///
    /// Gathers all [`MemoryKind::Fact`] and [`MemoryKind::Preference`] entries,
    /// sends them to the LLM with a structured extraction prompt, and merges the
    /// result with the existing profile. Resets the extraction counter on success.
    pub async fn extract_profile(
        &self,
        provider: &dyn aivyx_llm::LlmProvider,
    ) -> Result<crate::profile::UserProfile> {
        use crate::profile_extractor::{
            build_profile_extraction_request, parse_profile_extraction,
        };

        let current = self.get_profile()?;

        // Gather all Fact and Preference memories
        let memory_ids = self.store.list_memories()?;
        let mut facts_and_prefs = Vec::new();
        for id in &memory_ids {
            if let Some(entry) = self.store.load_memory(id, &self.master_key)? {
                match entry.kind {
                    MemoryKind::Fact | MemoryKind::Preference => {
                        facts_and_prefs.push(entry.content);
                    }
                    _ => {}
                }
            }
        }

        if facts_and_prefs.is_empty() {
            debug!("No facts or preferences to extract profile from");
            return Ok(current);
        }

        debug!(
            "Extracting profile from {} facts/preferences",
            facts_and_prefs.len()
        );

        let request = build_profile_extraction_request(&current, &facts_and_prefs);
        let response = provider.chat(&request).await?;
        let extracted = parse_profile_extraction(response.message.content.text(), &current)?;

        // Save the extracted profile (update_profile increments revision)
        self.update_profile(extracted)?;

        // Reset the counter since we just extracted
        self.reset_extraction_counter()?;

        // Re-load to get the updated revision and timestamp
        let saved = self.get_profile()?;
        debug!("Profile extraction complete (revision {})", saved.revision);
        Ok(saved)
    }

    /// Embed text, checking the content-hash cache first.
    async fn embed_with_cache(&self, text: &str) -> Result<Vec<f32>> {
        let hash = content_hash(text);

        // Cache hit?
        if let Some(cached) = self.store.get_cached_embedding(&hash, &self.master_key)? {
            debug!("Embedding cache hit for hash {}", &hash[..8]);
            return Ok(cached);
        }

        // Cache miss — call provider
        let embedding = self.embedding_provider.embed(text).await?;
        let vector = embedding.vector;

        // Cache the result
        self.store
            .cache_embedding(&hash, &vector, &self.master_key)?;
        debug!("Cached embedding for hash {}", &hash[..8]);

        Ok(vector)
    }
}

fn format_kind(kind: &MemoryKind) -> &str {
    match kind {
        MemoryKind::Fact => "fact",
        MemoryKind::Preference => "preference",
        MemoryKind::SessionSummary => "session",
        MemoryKind::Procedure => "procedure",
        MemoryKind::Decision => "decision",
        MemoryKind::Outcome => "outcome",
        MemoryKind::Custom(s) => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock embedding provider that returns deterministic vectors based on
    /// a simple hash of the input text. Tracks call count.
    struct MockEmbeddingProvider {
        dims: usize,
        call_count: AtomicUsize,
    }

    impl MockEmbeddingProvider {
        fn new(dims: usize) -> Self {
            Self {
                dims,
                call_count: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn dimensions(&self) -> usize {
            self.dims
        }

        async fn embed(&self, text: &str) -> Result<aivyx_llm::Embedding> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            // Generate a deterministic vector from the text
            let mut vector = vec![0.0_f32; self.dims];
            for (i, byte) in text.bytes().enumerate() {
                vector[i % self.dims] += byte as f32 / 255.0;
            }
            // Normalize
            let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut vector {
                    *v /= norm;
                }
            }
            Ok(aivyx_llm::Embedding {
                vector,
                dimensions: self.dims,
            })
        }
    }

    fn setup(
        dims: usize,
    ) -> (
        MemoryStore,
        Arc<MockEmbeddingProvider>,
        MasterKey,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!("aivyx-mgr-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db");
        let store = MemoryStore::open(&db_path).unwrap();
        let master_key = MasterKey::generate();
        let provider = Arc::new(MockEmbeddingProvider::new(dims));
        (store, provider, master_key, dir)
    }

    #[tokio::test]
    async fn remember_stores_and_embeds() {
        let (store, provider, key, dir) = setup(4);
        let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        let id = mgr
            .remember("Rust is fast".into(), MemoryKind::Fact, None, vec![])
            .await
            .unwrap();

        assert_eq!(provider.calls(), 1);
        let stats = mgr.stats().unwrap();
        assert_eq!(stats.total_memories, 1);
        assert_eq!(stats.index_size, 1);

        // Memory should be loadable
        let entry = mgr
            .store
            .load_memory(&id, &mgr.master_key)
            .unwrap()
            .unwrap();
        assert_eq!(entry.content, "Rust is fast");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn cache_hit_avoids_duplicate_embed() {
        let (store, provider, key, dir) = setup(4);
        let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        // Store same text twice — second is both an embedding cache hit
        // AND a content dedup hit (cosine similarity = 1.0 > 0.95)
        let id1 = mgr
            .remember("identical text".into(), MemoryKind::Fact, None, vec![])
            .await
            .unwrap();
        let id2 = mgr
            .remember("identical text".into(), MemoryKind::Fact, None, vec![])
            .await
            .unwrap();

        // Only one embed call (second was cached)
        assert_eq!(provider.calls(), 1);
        // Dedup: only one memory stored, second returns existing ID
        assert_eq!(mgr.stats().unwrap().total_memories, 1);
        assert_eq!(id1, id2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn recall_returns_relevant_sorted() {
        let (store, provider, key, dir) = setup(4);
        let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        mgr.remember(
            "Rust programming language".into(),
            MemoryKind::Fact,
            None,
            vec![],
        )
        .await
        .unwrap();
        mgr.remember(
            "Python scripting language".into(),
            MemoryKind::Fact,
            None,
            vec![],
        )
        .await
        .unwrap();
        mgr.remember(
            "Rust is fast and safe".into(),
            MemoryKind::Fact,
            None,
            vec![],
        )
        .await
        .unwrap();

        let results = mgr.recall("Rust programming", 2, None, &[]).await.unwrap();
        assert!(results.len() <= 2);
        // Results should be non-empty
        assert!(!results.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn recall_respects_agent_scope() {
        let (store, provider, key, dir) = setup(4);
        let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        // Private to agent A
        mgr.remember("secret A".into(), MemoryKind::Fact, Some(agent_a), vec![])
            .await
            .unwrap();
        // Global
        mgr.remember("global fact".into(), MemoryKind::Fact, None, vec![])
            .await
            .unwrap();

        // Agent B should only see the global fact
        let results = mgr.recall("secret", 10, Some(agent_b), &[]).await.unwrap();
        assert!(results.iter().all(|e| e.agent_scope.is_none()));

        // Agent A should see both
        let results = mgr.recall("secret", 10, Some(agent_a), &[]).await.unwrap();
        assert!(results.len() >= 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_and_query_triples() {
        let (store, provider, key, dir) = setup(4);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        mgr.add_triple(
            "Rust".into(),
            "is_a".into(),
            "language".into(),
            None,
            0.95,
            "user".into(),
        )
        .unwrap();
        mgr.add_triple(
            "Python".into(),
            "is_a".into(),
            "language".into(),
            None,
            0.9,
            "user".into(),
        )
        .unwrap();

        let all = mgr.query_triples(None, None, None, None).unwrap();
        assert_eq!(all.len(), 2);

        let rust_only = mgr.query_triples(Some("Rust"), None, None, None).unwrap();
        assert_eq!(rust_only.len(), 1);
        assert_eq!(rust_only[0].subject, "Rust");

        let by_pred = mgr.query_triples(None, Some("is_a"), None, None).unwrap();
        assert_eq!(by_pred.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn forget_removes_everything() {
        let (store, provider, key, dir) = setup(4);
        let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        let id = mgr
            .remember("to forget".into(), MemoryKind::Fact, None, vec![])
            .await
            .unwrap();

        assert_eq!(mgr.stats().unwrap().total_memories, 1);
        assert_eq!(mgr.stats().unwrap().index_size, 1);

        mgr.forget(&id).unwrap();

        assert_eq!(mgr.stats().unwrap().total_memories, 0);
        assert_eq!(mgr.stats().unwrap().index_size, 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_context_output() {
        let memories = vec![MemoryEntry::new(
            "Rust is fast".into(),
            MemoryKind::Fact,
            None,
            vec![],
        )];
        let triples = vec![KnowledgeTriple::new(
            "Rust".into(),
            "is_a".into(),
            "language".into(),
            None,
            0.95,
            "user".into(),
        )];

        let ctx = MemoryManager::format_context(&memories, &triples);
        assert!(ctx.starts_with("[MEMORY CONTEXT]"));
        assert!(ctx.ends_with("[END MEMORY CONTEXT]"));
        assert!(ctx.contains("Rust is fast"));
        assert!(ctx.contains("Rust is_a language"));
        assert!(ctx.contains("95%"));
    }

    #[test]
    fn stats_correct() {
        let (store, provider, key, dir) = setup(4);
        let mgr = MemoryManager::new(store, provider, key, 0).unwrap();
        let stats = mgr.stats().unwrap();
        assert_eq!(stats.total_memories, 0);
        assert_eq!(stats.total_triples, 0);
        assert_eq!(stats.index_size, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn new_builds_index_from_existing_data() {
        let (store, provider, key, dir) = setup(4);

        // Pre-populate the store
        let entry = MemoryEntry::new("existing".into(), MemoryKind::Fact, None, vec![]);
        let id = entry.id;
        store.save_memory(&entry, &key).unwrap();
        store
            .save_embedding(&id, &[0.1, 0.2, 0.3, 0.4], &key)
            .unwrap();

        // Create manager — should pick up existing data
        let mgr = MemoryManager::new(store, provider, key, 0).unwrap();
        assert_eq!(mgr.stats().unwrap().index_size, 1);
        assert_eq!(mgr.stats().unwrap().total_memories, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn remember_dedup_skips_near_duplicate() {
        let (store, provider, key, dir) = setup(4);
        let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        let id1 = mgr
            .remember("Rust is fast".into(), MemoryKind::Fact, None, vec![])
            .await
            .unwrap();
        // Identical text — should be deduplicated
        let id2 = mgr
            .remember("Rust is fast".into(), MemoryKind::Fact, None, vec![])
            .await
            .unwrap();

        assert_eq!(id1, id2, "duplicate should return existing ID");
        assert_eq!(mgr.stats().unwrap().total_memories, 1);
        assert_eq!(mgr.stats().unwrap().index_size, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_near_duplicate_with_controlled_vectors() {
        let (store, provider, key, dir) = setup(3);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Insert a vector pointing along the x-axis
        let id1 = MemoryId::new();
        mgr.index.upsert(id1, vec![1.0, 0.0, 0.0]);

        // A nearly identical vector should be found as a duplicate
        let near = vec![0.99, 0.01, 0.0];
        assert!(
            mgr.find_near_duplicate(&near, 0.95).is_some(),
            "near-identical vector should be detected"
        );

        // An orthogonal vector should NOT be a duplicate
        let orthogonal = vec![0.0, 1.0, 0.0];
        assert!(
            mgr.find_near_duplicate(&orthogonal, 0.95).is_none(),
            "orthogonal vector should not be a duplicate"
        );

        // Empty index should return None
        mgr.index.remove(&id1);
        assert!(mgr.find_near_duplicate(&near, 0.95).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn prune_removes_least_accessed() {
        let dir = std::env::temp_dir().join(format!("aivyx-prune-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = MemoryStore::open(dir.join("memory.db")).unwrap();
        let provider = Arc::new(MockEmbeddingProvider::new(128));
        let key = MasterKey::generate();
        // Limit to 3 memories
        let mut mgr = MemoryManager::new(store, provider.clone(), key, 3).unwrap();

        // Store memories with manually inserted entries to bypass dedup.
        // We create entries directly, embed them, then save — this way
        // we control the content and avoid mock embedder similarity issues.
        let entry1 = MemoryEntry::new("AAAA".repeat(50), MemoryKind::Fact, None, vec![]);
        let entry2 = MemoryEntry::new("ZZZZ".repeat(50), MemoryKind::Fact, None, vec![]);
        let entry3 = MemoryEntry::new("1234".repeat(50), MemoryKind::Fact, None, vec![]);
        let id1 = entry1.id;
        let id2 = entry2.id;
        let id3 = entry3.id;

        // Manually persist entries with distinct embeddings
        for (entry, vec) in [
            (&entry1, {
                let mut v = vec![0.0f32; 128];
                v[0] = 1.0;
                v
            }),
            (&entry2, {
                let mut v = vec![0.0f32; 128];
                v[1] = 1.0;
                v
            }),
            (&entry3, {
                let mut v = vec![0.0f32; 128];
                v[2] = 1.0;
                v
            }),
        ] {
            mgr.store.save_memory(entry, &mgr.master_key).unwrap();
            mgr.store
                .save_embedding(&entry.id, &vec, &mgr.master_key)
                .unwrap();
            mgr.index.upsert(entry.id, vec);
        }

        assert_eq!(mgr.stats().unwrap().total_memories, 3);

        // Access entry2 and entry3 to bump their access_count (load + save)
        {
            let mut e2 = mgr
                .store
                .load_memory(&id2, &mgr.master_key)
                .unwrap()
                .unwrap();
            e2.record_access();
            mgr.store.save_memory(&e2, &mgr.master_key).unwrap();
        }
        {
            let mut e3 = mgr
                .store
                .load_memory(&id3, &mgr.master_key)
                .unwrap()
                .unwrap();
            e3.record_access();
            mgr.store.save_memory(&e3, &mgr.master_key).unwrap();
        }

        // Store a 4th memory via direct insert with orthogonal vector
        let entry4 = MemoryEntry::new("!@#$".repeat(50), MemoryKind::Fact, None, vec![]);
        let id4 = entry4.id;
        let v4 = {
            let mut v = vec![0.0f32; 128];
            v[3] = 1.0;
            v
        };
        mgr.store.save_memory(&entry4, &mgr.master_key).unwrap();
        mgr.store
            .save_embedding(&id4, &v4, &mgr.master_key)
            .unwrap();
        mgr.index.upsert(id4, v4);

        // Now trigger pruning explicitly
        mgr.prune_to_limit().unwrap();

        assert_eq!(
            mgr.stats().unwrap().total_memories,
            3,
            "should have pruned back to limit"
        );

        // id1 (never accessed, access_count=0) should be gone
        assert!(
            mgr.store
                .load_memory(&id1, &mgr.master_key)
                .unwrap()
                .is_none(),
            "least-accessed memory should be pruned"
        );

        // id2, id3, id4 should survive (id2 and id3 have access_count=1, id4 is new but
        // only 1 entry needed to be removed)
        assert!(
            mgr.store
                .load_memory(&id2, &mgr.master_key)
                .unwrap()
                .is_some()
        );
        assert!(
            mgr.store
                .load_memory(&id3, &mgr.master_key)
                .unwrap()
                .is_some()
        );
        assert!(
            mgr.store
                .load_memory(&id4, &mgr.master_key)
                .unwrap()
                .is_some()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn prune_unlimited_does_nothing() {
        let dir = std::env::temp_dir().join(format!("aivyx-unlim-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = MemoryStore::open(dir.join("memory.db")).unwrap();
        let provider = Arc::new(MockEmbeddingProvider::new(128));
        let key = MasterKey::generate();
        // max_memories = 0 means unlimited
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Directly insert 10 entries with orthogonal vectors to bypass dedup
        for i in 0..10 {
            let entry = MemoryEntry::new(
                format!("Memory content #{i}"),
                MemoryKind::Fact,
                None,
                vec![],
            );
            let mut vec = vec![0.0f32; 128];
            vec[i] = 1.0; // each vector points along a unique axis
            mgr.store.save_memory(&entry, &mgr.master_key).unwrap();
            mgr.store
                .save_embedding(&entry.id, &vec, &mgr.master_key)
                .unwrap();
            mgr.index.upsert(entry.id, vec);
        }

        assert_eq!(
            mgr.stats().unwrap().total_memories,
            10,
            "unlimited mode should never prune"
        );

        // Pruning call should be a no-op
        mgr.prune_to_limit().unwrap();
        assert_eq!(mgr.stats().unwrap().total_memories, 10);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_profile_returns_empty_when_none() {
        let (store, provider, key, dir) = setup(4);
        let mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        let profile = mgr.get_profile().unwrap();
        assert!(profile.is_empty());
        assert_eq!(profile.revision, 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_profile_increments_revision() {
        let (store, provider, key, dir) = setup(4);
        let mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        let mut profile = crate::profile::UserProfile::new();
        profile.name = Some("Julian".into());

        mgr.update_profile(profile).unwrap();

        let loaded = mgr.get_profile().unwrap();
        assert_eq!(loaded.name.as_deref(), Some("Julian"));
        assert_eq!(loaded.revision, 1);

        // Update again
        let mut profile2 = loaded;
        profile2.timezone = Some("UTC".into());
        mgr.update_profile(profile2).unwrap();

        let loaded2 = mgr.get_profile().unwrap();
        assert_eq!(loaded2.revision, 2);
        assert_eq!(loaded2.timezone.as_deref(), Some("UTC"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_and_query_outcomes() {
        let (store, provider, key, dir) = setup(4);
        let mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        let r1 = crate::outcome::OutcomeRecord::new(
            crate::outcome::OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            true,
            "Command ran successfully".into(),
            250,
            "agent-1".into(),
            "run tests".into(),
        );
        let r2 = crate::outcome::OutcomeRecord::new(
            crate::outcome::OutcomeSource::Delegation {
                specialist: "coder".into(),
                task: "write feature".into(),
            },
            false,
            "Timeout".into(),
            30000,
            "lead".into(),
            "build feature".into(),
        );

        let id1 = mgr.record_outcome(&r1).unwrap();
        let id2 = mgr.record_outcome(&r2).unwrap();
        assert_eq!(id1, r1.id);
        assert_eq!(id2, r2.id);

        // Query all
        let all = mgr.query_outcomes(&OutcomeFilter::default()).unwrap();
        assert_eq!(all.len(), 2);

        // Query successes only
        let successes = mgr
            .query_outcomes(&OutcomeFilter {
                success: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(successes.len(), 1);
        assert_eq!(successes[0].id, r1.id);

        // Query by agent
        let lead_outcomes = mgr
            .query_outcomes(&OutcomeFilter {
                agent_name: Some("lead".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(lead_outcomes.len(), 1);
        assert!(!lead_outcomes[0].success);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extraction_counter_lifecycle() {
        let (store, provider, key, dir) = setup(4);
        let mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Starts at 0
        let c1 = mgr.increment_extraction_counter().unwrap();
        assert_eq!(c1, 1);

        let c2 = mgr.increment_extraction_counter().unwrap();
        assert_eq!(c2, 2);

        // Reset
        mgr.reset_extraction_counter().unwrap();

        let c3 = mgr.increment_extraction_counter().unwrap();
        assert_eq!(c3, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_kind_returns_correct_strings() {
        assert_eq!(format_kind(&MemoryKind::Fact), "fact");
        assert_eq!(format_kind(&MemoryKind::Preference), "preference");
        assert_eq!(format_kind(&MemoryKind::SessionSummary), "session");
        assert_eq!(format_kind(&MemoryKind::Procedure), "procedure");
        assert_eq!(format_kind(&MemoryKind::Decision), "decision");
        assert_eq!(format_kind(&MemoryKind::Outcome), "outcome");
        assert_eq!(
            format_kind(&MemoryKind::Custom("custom-kind".into())),
            "custom-kind"
        );
    }
}
