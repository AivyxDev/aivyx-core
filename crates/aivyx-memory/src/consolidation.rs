//! Memory consolidation: merge similar memories, prune stale ones, and
//! strengthen frequently accessed entries.

use tracing::debug;

use aivyx_core::{MemoryId, Result};
use aivyx_llm::LlmProvider;

use crate::manager::MemoryManager;
use crate::search::cosine_similarity;
use crate::types::MemoryEntry;

/// Configuration for memory consolidation.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Cosine similarity threshold for merging related memories.
    pub merge_threshold: f32,
    /// Number of days after which an unaccessed memory is considered stale.
    pub stale_days: u64,
    /// Maximum number of memories to process per consolidation run.
    pub batch_size: usize,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            merge_threshold: 0.85,
            stale_days: 90,
            batch_size: 200,
        }
    }
}

/// Report summarizing what a consolidation run accomplished.
#[derive(Debug, Clone, Default)]
pub struct ConsolidationReport {
    /// Number of memory clusters that were merged.
    pub clusters_merged: usize,
    /// Number of stale memories that were pruned.
    pub memories_pruned: usize,
    /// Number of high-access memories that were strengthened with a tag.
    pub memories_strengthened: usize,
}

/// Run memory consolidation: merge similar memories via LLM, prune stale
/// unaccessed entries, and strengthen frequently accessed ones.
pub async fn consolidate(
    manager: &mut MemoryManager,
    provider: &dyn LlmProvider,
    config: &ConsolidationConfig,
) -> Result<ConsolidationReport> {
    let mut report = ConsolidationReport::default();

    // 1. Load all memories (up to batch_size)
    let memory_ids = manager.store.list_memories()?;
    let mut entries: Vec<MemoryEntry> = Vec::new();
    for id in memory_ids.iter().take(config.batch_size) {
        if let Some(entry) = manager.store.load_memory(id, &manager.master_key)? {
            entries.push(entry);
        }
    }

    // 2. Collect vectors for pairwise comparison
    let all_vectors = manager.index.all_vectors();
    let entry_ids: Vec<MemoryId> = entries.iter().map(|e| e.id).collect();

    // 3. Greedy clustering by cosine similarity
    let mut clustered: Vec<bool> = vec![false; entries.len()];
    let mut clusters: Vec<Vec<usize>> = Vec::new();

    for i in 0..entries.len() {
        if clustered[i] {
            continue;
        }
        let vec_i = match all_vectors.get(&entry_ids[i]) {
            Some(v) => v,
            None => continue,
        };

        let mut cluster = vec![i];
        clustered[i] = true;

        for j in (i + 1)..entries.len() {
            if clustered[j] {
                continue;
            }
            let vec_j = match all_vectors.get(&entry_ids[j]) {
                Some(v) => v,
                None => continue,
            };

            if cosine_similarity(vec_i, vec_j) >= config.merge_threshold {
                cluster.push(j);
                clustered[j] = true;
            }
        }

        if cluster.len() >= 2 {
            clusters.push(cluster);
        }
    }

    // 4. Merge each cluster via LLM
    for cluster in &clusters {
        let contents: Vec<&str> = cluster
            .iter()
            .map(|&idx| entries[idx].content.as_str())
            .collect();

        let merged_content = llm_merge(provider, &contents).await?;

        // Use the kind of the first entry in the cluster
        let kind = entries[cluster[0]].kind.clone();
        let agent_scope = entries[cluster[0]].agent_scope;

        // Collect all tags from the cluster (deduplicated)
        let mut all_tags: Vec<String> = cluster
            .iter()
            .flat_map(|&idx| entries[idx].tags.iter().cloned())
            .collect();
        all_tags.sort();
        all_tags.dedup();

        // Delete originals
        for &idx in cluster {
            manager.forget(&entries[idx].id)?;
        }

        // Store the merged memory
        manager
            .remember(merged_content, kind, agent_scope, all_tags)
            .await?;

        report.clusters_merged += 1;
    }

    // 5. Re-load entries for decay and strengthening (state changed after merges)
    let memory_ids = manager.store.list_memories()?;
    let now = chrono::Utc::now();
    let stale_cutoff = now - chrono::Duration::days(config.stale_days as i64);

    for id in &memory_ids {
        if let Some(mut entry) = manager.store.load_memory(id, &manager.master_key)? {
            // Decay: prune unaccessed memories older than stale_days
            if entry.access_count == 0 && entry.created_at < stale_cutoff {
                manager.forget(&entry.id)?;
                report.memories_pruned += 1;
                continue;
            }

            // Strengthen: high access_count memories get a tag
            if entry.access_count >= 5 && !entry.tags.contains(&"high-confidence".to_string()) {
                entry.tags.push("high-confidence".to_string());
                entry.updated_at = now;
                manager.store.save_memory(&entry, &manager.master_key)?;
                report.memories_strengthened += 1;
            }
        }
    }

    debug!(
        "Consolidation complete: {} clusters merged, {} pruned, {} strengthened",
        report.clusters_merged, report.memories_pruned, report.memories_strengthened,
    );

    Ok(report)
}

/// Use the LLM to merge multiple related memory contents into a single summary.
async fn llm_merge(provider: &dyn LlmProvider, contents: &[&str]) -> Result<String> {
    use aivyx_llm::message::{ChatMessage, ChatRequest};

    let numbered: Vec<String> = contents
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}. {}", i + 1, c))
        .collect();

    let user_msg = format!(
        "Combine these related memories into a single concise summary:\n\n{}",
        numbered.join("\n")
    );

    let request = ChatRequest {
        system_prompt: Some(
            "You are a memory consolidation assistant. Combine the given related \
             memories into a single, concise summary that preserves all important \
             information. Return only the merged summary, nothing else."
                .into(),
        ),
        messages: vec![ChatMessage::user(user_msg)],
        tools: vec![],
        model: None,
        max_tokens: 1024,
    };

    let response = provider.chat(&request).await?;
    Ok(response.message.content.text().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;
    use crate::types::MemoryKind;
    use aivyx_crypto::MasterKey;
    use aivyx_llm::EmbeddingProvider;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock embedding provider that returns deterministic vectors.
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
            let mut vector = vec![0.0_f32; self.dims];
            for (i, byte) in text.bytes().enumerate() {
                vector[i % self.dims] += byte as f32 / 255.0;
            }
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

    /// A mock LLM provider that concatenates inputs for merge.
    struct MockLlmProvider;

    #[async_trait]
    impl LlmProvider for MockLlmProvider {
        fn name(&self) -> &str {
            "mock-llm"
        }

        async fn chat(
            &self,
            request: &aivyx_llm::message::ChatRequest,
        ) -> Result<aivyx_llm::message::ChatResponse> {
            // Extract the user message and return a simple merge
            let user_text = request.messages[0].content.text();
            Ok(aivyx_llm::message::ChatResponse {
                message: aivyx_llm::message::ChatMessage::assistant(format!(
                    "Merged: {}",
                    user_text.lines().count()
                )),
                usage: aivyx_llm::message::TokenUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
                stop_reason: aivyx_llm::message::StopReason::EndTurn,
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
        let dir =
            std::env::temp_dir().join(format!("aivyx-consolidation-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db");
        let store = MemoryStore::open(&db_path).unwrap();
        let master_key = MasterKey::generate();
        let provider = Arc::new(MockEmbeddingProvider::new(dims));
        (store, provider, master_key, dir)
    }

    #[test]
    fn consolidation_config_defaults() {
        let config = ConsolidationConfig::default();
        assert!((config.merge_threshold - 0.85).abs() < f32::EPSILON);
        assert_eq!(config.stale_days, 90);
        assert_eq!(config.batch_size, 200);
    }

    #[test]
    fn consolidation_report_defaults() {
        let report = ConsolidationReport::default();
        assert_eq!(report.clusters_merged, 0);
        assert_eq!(report.memories_pruned, 0);
        assert_eq!(report.memories_strengthened, 0);
    }

    #[tokio::test]
    async fn cluster_detection_merges_similar() {
        let (store, provider, key, dir) = setup(128);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Insert 3 memories with nearly identical vectors (along axis 0)
        // to ensure they cluster together
        let e1 = MemoryEntry::new(
            "Rust is fast".into(),
            MemoryKind::Fact,
            None,
            vec!["lang".into()],
        );
        let e2 = MemoryEntry::new(
            "Rust is speedy".into(),
            MemoryKind::Fact,
            None,
            vec!["lang".into()],
        );
        let e3 = MemoryEntry::new(
            "Rust is quick".into(),
            MemoryKind::Fact,
            None,
            vec!["perf".into()],
        );

        // Use nearly identical vectors so cosine similarity > 0.85
        let v1 = {
            let mut v = vec![0.0f32; 128];
            v[0] = 1.0;
            v
        };
        let v2 = {
            let mut v = vec![0.0f32; 128];
            v[0] = 0.99;
            v[1] = 0.01;
            v
        };
        let v3 = {
            let mut v = vec![0.0f32; 128];
            v[0] = 0.98;
            v[1] = 0.02;
            v
        };

        mgr.store_raw(&e1, &v1).unwrap();
        mgr.store_raw(&e2, &v2).unwrap();
        mgr.store_raw(&e3, &v3).unwrap();

        assert_eq!(mgr.stats().unwrap().total_memories, 3);

        let config = ConsolidationConfig::default();
        let llm = MockLlmProvider;
        let report = consolidate(&mut mgr, &llm, &config).await.unwrap();

        assert_eq!(
            report.clusters_merged, 1,
            "3 similar memories should form 1 cluster"
        );
        // After merging, we should have fewer memories (the 3 originals replaced by 1 merged)
        assert!(
            mgr.stats().unwrap().total_memories < 3,
            "merged memories should reduce total count"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn decay_prunes_old_unaccessed() {
        let (store, provider, key, dir) = setup(128);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Insert an old memory with access_count == 0
        let mut old_entry = MemoryEntry::new("Ancient fact".into(), MemoryKind::Fact, None, vec![]);
        // Set created_at to 100 days ago
        old_entry.created_at = chrono::Utc::now() - chrono::Duration::days(100);
        old_entry.updated_at = old_entry.created_at;
        let old_id = old_entry.id;

        let v = {
            let mut v = vec![0.0f32; 128];
            v[0] = 1.0;
            v
        };
        mgr.store_raw(&old_entry, &v).unwrap();

        // Insert a recent memory
        let recent = MemoryEntry::new("Recent fact".into(), MemoryKind::Fact, None, vec![]);
        let recent_id = recent.id;
        let v2 = {
            let mut v = vec![0.0f32; 128];
            v[1] = 1.0;
            v
        };
        mgr.store_raw(&recent, &v2).unwrap();

        assert_eq!(mgr.stats().unwrap().total_memories, 2);

        let config = ConsolidationConfig {
            stale_days: 90,
            merge_threshold: 0.99, // high threshold to avoid merging
            ..Default::default()
        };
        let llm = MockLlmProvider;
        let report = consolidate(&mut mgr, &llm, &config).await.unwrap();

        assert_eq!(report.memories_pruned, 1);
        // Old memory should be gone
        assert!(
            mgr.store
                .load_memory(&old_id, &mgr.master_key)
                .unwrap()
                .is_none()
        );
        // Recent memory should remain
        assert!(
            mgr.store
                .load_memory(&recent_id, &mgr.master_key)
                .unwrap()
                .is_some()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn strengthen_high_access() {
        let (store, provider, key, dir) = setup(128);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Insert a memory with high access count
        let mut entry = MemoryEntry::new("Important fact".into(), MemoryKind::Fact, None, vec![]);
        for _ in 0..5 {
            entry.record_access();
        }
        let id = entry.id;

        let v = {
            let mut v = vec![0.0f32; 128];
            v[0] = 1.0;
            v
        };
        mgr.store_raw(&entry, &v).unwrap();

        let config = ConsolidationConfig {
            merge_threshold: 0.99, // high threshold to avoid merging
            ..Default::default()
        };
        let llm = MockLlmProvider;
        let report = consolidate(&mut mgr, &llm, &config).await.unwrap();

        assert_eq!(report.memories_strengthened, 1);
        let updated = mgr
            .store
            .load_memory(&id, &mgr.master_key)
            .unwrap()
            .unwrap();
        assert!(
            updated.tags.contains(&"high-confidence".to_string()),
            "high-access memory should get high-confidence tag"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn strengthen_idempotent() {
        let (store, provider, key, dir) = setup(128);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Insert a memory already tagged high-confidence with high access count
        let mut entry = MemoryEntry::new(
            "Already strong".into(),
            MemoryKind::Fact,
            None,
            vec!["high-confidence".into()],
        );
        for _ in 0..10 {
            entry.record_access();
        }

        let v = {
            let mut v = vec![0.0f32; 128];
            v[0] = 1.0;
            v
        };
        mgr.store_raw(&entry, &v).unwrap();

        let config = ConsolidationConfig {
            merge_threshold: 0.99,
            ..Default::default()
        };
        let llm = MockLlmProvider;
        let report = consolidate(&mut mgr, &llm, &config).await.unwrap();

        // Should not be counted as strengthened since it already has the tag
        assert_eq!(report.memories_strengthened, 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn consolidation_report_fields() {
        let (store, provider, key, dir) = setup(128);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Insert 2 similar memories (will merge), 1 old (will prune),
        // 1 high-access (will strengthen)
        let e1 = MemoryEntry::new("Similar A".into(), MemoryKind::Fact, None, vec![]);
        let e2 = MemoryEntry::new("Similar B".into(), MemoryKind::Fact, None, vec![]);

        // Nearly identical vectors for clustering
        let v1 = {
            let mut v = vec![0.0f32; 128];
            v[0] = 1.0;
            v
        };
        let v2 = {
            let mut v = vec![0.0f32; 128];
            v[0] = 0.99;
            v[1] = 0.01;
            v
        };
        mgr.store_raw(&e1, &v1).unwrap();
        mgr.store_raw(&e2, &v2).unwrap();

        // Old unaccessed memory
        let mut old = MemoryEntry::new("Old fact".into(), MemoryKind::Fact, None, vec![]);
        old.created_at = chrono::Utc::now() - chrono::Duration::days(100);
        old.updated_at = old.created_at;
        let v_old = {
            let mut v = vec![0.0f32; 128];
            v[2] = 1.0;
            v
        };
        mgr.store_raw(&old, &v_old).unwrap();

        // High-access memory
        let mut strong = MemoryEntry::new("Strong fact".into(), MemoryKind::Fact, None, vec![]);
        for _ in 0..6 {
            strong.record_access();
        }
        let v_strong = {
            let mut v = vec![0.0f32; 128];
            v[3] = 1.0;
            v
        };
        mgr.store_raw(&strong, &v_strong).unwrap();

        let config = ConsolidationConfig::default();
        let llm = MockLlmProvider;
        let report = consolidate(&mut mgr, &llm, &config).await.unwrap();

        assert_eq!(report.clusters_merged, 1);
        assert_eq!(report.memories_pruned, 1);
        assert_eq!(report.memories_strengthened, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn no_merge_below_threshold() {
        let (store, provider, key, dir) = setup(128);
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Insert 2 memories with orthogonal vectors (similarity = 0)
        let e1 = MemoryEntry::new("Topic A".into(), MemoryKind::Fact, None, vec![]);
        let e2 = MemoryEntry::new("Topic B".into(), MemoryKind::Fact, None, vec![]);

        let v1 = {
            let mut v = vec![0.0f32; 128];
            v[0] = 1.0;
            v
        };
        let v2 = {
            let mut v = vec![0.0f32; 128];
            v[1] = 1.0;
            v
        };
        mgr.store_raw(&e1, &v1).unwrap();
        mgr.store_raw(&e2, &v2).unwrap();

        let config = ConsolidationConfig {
            merge_threshold: 0.85,
            ..Default::default()
        };
        let llm = MockLlmProvider;
        let report = consolidate(&mut mgr, &llm, &config).await.unwrap();

        assert_eq!(report.clusters_merged, 0);
        assert_eq!(mgr.stats().unwrap().total_memories, 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}
