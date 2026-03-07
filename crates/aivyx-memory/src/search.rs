//! Vector search engine with cosine similarity.
//!
//! Provides an in-memory vector index that loads from [`MemoryStore`] on startup
//! and supports upsert, remove, and top-k similarity search.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use aivyx_core::{MemoryId, Result};
use aivyx_crypto::MasterKey;

use crate::store::MemoryStore;

/// A search result with memory ID and similarity score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The memory ID that matched.
    pub memory_id: MemoryId,
    /// Cosine similarity score (higher = more similar).
    pub similarity: f32,
}

/// In-memory vector index for cosine similarity search.
pub struct VectorIndex {
    vectors: HashMap<MemoryId, Vec<f32>>,
}

impl VectorIndex {
    /// Create an empty index.
    pub fn new() -> Self {
        Self {
            vectors: HashMap::new(),
        }
    }

    /// Build an index from all embeddings in the store.
    pub fn build(store: &MemoryStore, master_key: &MasterKey) -> Result<Self> {
        let ids = store.list_memories()?;
        let mut vectors = HashMap::new();

        for id in ids {
            if let Some(vec) = store.load_embedding(&id, master_key)? {
                vectors.insert(id, vec);
            }
        }

        Ok(Self { vectors })
    }

    /// Insert or update an embedding vector.
    pub fn upsert(&mut self, id: MemoryId, vector: Vec<f32>) {
        self.vectors.insert(id, vector);
    }

    /// Remove an embedding from the index.
    pub fn remove(&mut self, id: &MemoryId) {
        self.vectors.remove(id);
    }

    /// Search for the top-k most similar vectors to the query.
    pub fn search(&self, query: &[f32], top_k: usize) -> Vec<SearchResult> {
        let mut results: Vec<SearchResult> = self
            .vectors
            .iter()
            .map(|(id, vec)| SearchResult {
                memory_id: *id,
                similarity: cosine_similarity(query, vec),
            })
            .collect();

        // Sort by similarity descending.
        results.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k);
        results
    }

    /// Return a reference to all stored vectors, keyed by memory ID.
    pub fn all_vectors(&self) -> &HashMap<MemoryId, Vec<f32>> {
        &self.vectors
    }

    /// Number of vectors in the index.
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
}

impl Default for VectorIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute cosine similarity between two vectors.
///
/// Returns 0.0 if either vector has zero magnitude (avoiding division by zero).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

/// Compute a SHA-256 hex digest of the given text, for use as an embedding cache key.
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn cosine_zero_vector() {
        let a = vec![1.0, 2.0];
        let b = vec![0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn cosine_mismatched_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn cosine_empty_vectors() {
        let sim = cosine_similarity(&[], &[]);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn content_hash_deterministic() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex is 64 chars
    }

    #[test]
    fn content_hash_differs_for_different_inputs() {
        let h1 = content_hash("hello");
        let h2 = content_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn vector_index_upsert_and_search() {
        let mut index = VectorIndex::new();
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();
        let id3 = MemoryId::new();

        // id1 and id3 are similar to each other, id2 is different
        index.upsert(id1, vec![1.0, 0.0, 0.0]);
        index.upsert(id2, vec![0.0, 1.0, 0.0]);
        index.upsert(id3, vec![0.9, 0.1, 0.0]);

        let results = index.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        // id1 should be most similar (identical)
        assert_eq!(results[0].memory_id, id1);
        assert!((results[0].similarity - 1.0).abs() < 1e-6);
        // id3 should be second
        assert_eq!(results[1].memory_id, id3);
    }

    #[test]
    fn vector_index_remove() {
        let mut index = VectorIndex::new();
        let id = MemoryId::new();
        index.upsert(id, vec![1.0, 2.0]);
        assert_eq!(index.len(), 1);

        index.remove(&id);
        assert_eq!(index.len(), 0);
        assert!(index.is_empty());
    }

    #[test]
    fn vector_index_build_from_store() {
        let dir = std::env::temp_dir().join(format!("aivyx-vidx-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db");
        let store = MemoryStore::open(&db_path).unwrap();
        let master_key = MasterKey::generate();

        // Store a memory and its embedding
        let entry = crate::types::MemoryEntry::new(
            "test".into(),
            crate::types::MemoryKind::Fact,
            None,
            vec![],
        );
        let id = entry.id;
        store.save_memory(&entry, &master_key).unwrap();
        store
            .save_embedding(&id, &[0.5, 0.6, 0.7], &master_key)
            .unwrap();

        let index = VectorIndex::build(&store, &master_key).unwrap();
        assert_eq!(index.len(), 1);

        let results = index.search(&[0.5, 0.6, 0.7], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_id, id);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn vector_index_search_top_k_limits() {
        let mut index = VectorIndex::new();
        for _ in 0..10 {
            index.upsert(MemoryId::new(), vec![1.0, 0.0]);
        }
        let results = index.search(&[1.0, 0.0], 3);
        assert_eq!(results.len(), 3);
    }
}
