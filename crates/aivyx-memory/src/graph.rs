//! In-memory knowledge graph built from stored triples.
//!
//! Provides BFS traversal, path finding, community detection, and entity
//! search over the knowledge triple store. The graph is built at startup and
//! kept in sync as triples are added.

use std::collections::{HashMap, HashSet, VecDeque};

use aivyx_core::{Result, TripleId};
use aivyx_crypto::MasterKey;

use crate::store::MemoryStore;
use crate::types::KnowledgeTriple;

/// An edge in the knowledge graph.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// ID of the underlying triple.
    pub triple_id: TripleId,
    /// The relationship (predicate).
    pub predicate: String,
    /// The target entity (object for outbound, subject for inbound).
    pub target: String,
    /// Confidence score from the source triple.
    pub confidence: f32,
}

/// A path through the knowledge graph.
#[derive(Debug, Clone)]
pub struct GraphPath {
    /// Sequence of (subject, predicate, object) hops.
    pub hops: Vec<(String, String, String)>,
}

/// A connected component (community) in the graph.
#[derive(Debug, Clone)]
pub struct Community {
    /// All entities in this connected component.
    pub entities: HashSet<String>,
    /// Number of edges within this community.
    pub edge_count: usize,
}

/// Neighborhood around an entity.
#[derive(Debug, Clone)]
pub struct Neighborhood {
    /// The central entity.
    pub entity: String,
    /// Edges going out from this entity (entity is subject).
    pub outbound: Vec<GraphEdge>,
    /// Edges coming into this entity (entity is object).
    pub inbound: Vec<GraphEdge>,
}

/// In-memory knowledge graph built from stored triples.
pub struct KnowledgeGraph {
    /// subject -> outbound edges
    adjacency: HashMap<String, Vec<GraphEdge>>,
    /// object -> inbound edges
    reverse: HashMap<String, Vec<GraphEdge>>,
    /// All known entities (subjects and objects).
    entities: HashSet<String>,
}

impl KnowledgeGraph {
    /// Build from all triples in the store.
    pub fn build(store: &MemoryStore, master_key: &MasterKey) -> Result<Self> {
        let mut graph = Self {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        let ids = store.list_triples()?;
        for id in ids {
            if let Some(triple) = store.load_triple(&id, master_key)? {
                graph.upsert_triple(&triple);
            }
        }

        Ok(graph)
    }

    /// Insert or update a triple in the graph.
    pub fn upsert_triple(&mut self, triple: &KnowledgeTriple) {
        self.entities.insert(triple.subject.clone());
        self.entities.insert(triple.object.clone());

        // Outbound edge: subject -> object
        let outbound = GraphEdge {
            triple_id: triple.id,
            predicate: triple.predicate.clone(),
            target: triple.object.clone(),
            confidence: triple.confidence,
        };
        self.adjacency
            .entry(triple.subject.clone())
            .or_default()
            .push(outbound);

        // Inbound edge: object <- subject
        let inbound = GraphEdge {
            triple_id: triple.id,
            predicate: triple.predicate.clone(),
            target: triple.subject.clone(),
            confidence: triple.confidence,
        };
        self.reverse
            .entry(triple.object.clone())
            .or_default()
            .push(inbound);
    }

    /// BFS traversal from entity up to `max_hops`, returning all paths found.
    pub fn traverse(&self, entity: &str, max_hops: usize) -> Vec<GraphPath> {
        if !self.entities.contains(entity) {
            return Vec::new();
        }

        let mut paths = Vec::new();
        // (current_entity, current_path_hops)
        let mut queue: VecDeque<(String, Vec<(String, String, String)>)> = VecDeque::new();
        let mut visited = HashSet::new();

        visited.insert(entity.to_string());
        queue.push_back((entity.to_string(), Vec::new()));

        while let Some((current, current_path)) = queue.pop_front() {
            if current_path.len() >= max_hops {
                continue;
            }

            if let Some(edges) = self.adjacency.get(&current) {
                for edge in edges {
                    let mut new_path = current_path.clone();
                    new_path.push((
                        current.clone(),
                        edge.predicate.clone(),
                        edge.target.clone(),
                    ));
                    paths.push(GraphPath {
                        hops: new_path.clone(),
                    });

                    if !visited.contains(&edge.target) {
                        visited.insert(edge.target.clone());
                        queue.push_back((edge.target.clone(), new_path));
                    }
                }
            }
        }

        paths
    }

    /// Find all paths between two entities using BFS, up to `max_depth`.
    pub fn find_paths(&self, from: &str, to: &str, max_depth: usize) -> Vec<GraphPath> {
        if !self.entities.contains(from) || !self.entities.contains(to) {
            return Vec::new();
        }

        let mut results = Vec::new();
        // (current_entity, path_so_far, visited_set)
        let mut queue: VecDeque<(String, Vec<(String, String, String)>, HashSet<String>)> =
            VecDeque::new();

        let mut initial_visited = HashSet::new();
        initial_visited.insert(from.to_string());
        queue.push_back((from.to_string(), Vec::new(), initial_visited));

        while let Some((current, path, visited)) = queue.pop_front() {
            if path.len() >= max_depth {
                continue;
            }

            if let Some(edges) = self.adjacency.get(&current) {
                for edge in edges {
                    let mut new_path = path.clone();
                    new_path.push((
                        current.clone(),
                        edge.predicate.clone(),
                        edge.target.clone(),
                    ));

                    if edge.target == to {
                        results.push(GraphPath { hops: new_path });
                        continue;
                    }

                    if !visited.contains(&edge.target) {
                        let mut new_visited = visited.clone();
                        new_visited.insert(edge.target.clone());
                        queue.push_back((edge.target.clone(), new_path, new_visited));
                    }
                }
            }
        }

        results
    }

    /// Detect connected components using BFS.
    pub fn detect_communities(&self) -> Vec<Community> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut communities = Vec::new();

        for entity in &self.entities {
            if visited.contains(entity) {
                continue;
            }

            let mut component = HashSet::new();
            let mut queue = VecDeque::new();
            queue.push_back(entity.clone());

            while let Some(current) = queue.pop_front() {
                if !component.insert(current.clone()) {
                    continue;
                }
                visited.insert(current.clone());

                // Follow outbound edges
                if let Some(edges) = self.adjacency.get(&current) {
                    for edge in edges {
                        if !component.contains(&edge.target) {
                            queue.push_back(edge.target.clone());
                        }
                    }
                }
                // Follow inbound edges (undirected connectivity)
                if let Some(edges) = self.reverse.get(&current) {
                    for edge in edges {
                        if !component.contains(&edge.target) {
                            queue.push_back(edge.target.clone());
                        }
                    }
                }
            }

            // Count edges within this component
            let mut edge_count = 0;
            for member in &component {
                if let Some(edges) = self.adjacency.get(member) {
                    edge_count += edges.len();
                }
            }

            communities.push(Community {
                entities: component,
                edge_count,
            });
        }

        communities
    }

    /// Get immediate neighborhood of an entity.
    pub fn neighborhood(&self, entity: &str) -> Neighborhood {
        let outbound = self
            .adjacency
            .get(entity)
            .cloned()
            .unwrap_or_default();
        let inbound = self
            .reverse
            .get(entity)
            .cloned()
            .unwrap_or_default();

        Neighborhood {
            entity: entity.to_string(),
            outbound,
            inbound,
        }
    }

    /// Case-insensitive entity search (substring match).
    ///
    /// Returns matches with a relevance score: 1.0 for exact match,
    /// `query.len() / entity.len()` for partial matches.
    pub fn search_entities(&self, query: &str) -> Vec<(String, f32)> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for entity in &self.entities {
            let entity_lower = entity.to_lowercase();
            if entity_lower == query_lower {
                results.push((entity.clone(), 1.0));
            } else if entity_lower.contains(&query_lower) {
                let score = query.len() as f32 / entity.len() as f32;
                results.push((entity.clone(), score));
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// Number of entities in the graph.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Number of edges (outbound) in the graph.
    pub fn edge_count(&self) -> usize {
        self.adjacency.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::KnowledgeTriple;

    fn make_triple(subject: &str, predicate: &str, object: &str) -> KnowledgeTriple {
        KnowledgeTriple::new(
            subject.into(),
            predicate.into(),
            object.into(),
            None,
            0.9,
            "test".into(),
        )
    }

    #[test]
    fn build_empty_graph() {
        let graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };
        assert_eq!(graph.entity_count(), 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn upsert_triple_updates_adjacency() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        let triple = make_triple("Rust", "is_a", "language");
        graph.upsert_triple(&triple);

        assert_eq!(graph.entity_count(), 2);
        assert_eq!(graph.edge_count(), 1);
        assert!(graph.entities.contains("Rust"));
        assert!(graph.entities.contains("language"));

        let edges = graph.adjacency.get("Rust").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target, "language");
        assert_eq!(edges[0].predicate, "is_a");
    }

    #[test]
    fn traverse_one_hop() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("A", "knows", "B"));
        graph.upsert_triple(&make_triple("A", "likes", "C"));

        let paths = graph.traverse("A", 1);
        assert_eq!(paths.len(), 2);
        // Both paths should start from A
        for path in &paths {
            assert_eq!(path.hops.len(), 1);
            assert_eq!(path.hops[0].0, "A");
        }
    }

    #[test]
    fn traverse_two_hops() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("A", "knows", "B"));
        graph.upsert_triple(&make_triple("B", "knows", "C"));

        let paths = graph.traverse("A", 2);
        // Should find: A->B (1 hop) and A->B->C (path from B, 1 hop)
        // Actually: A->B at depth 1, then B->C at depth 2
        assert!(paths.len() >= 2);

        // There should be a 2-hop path A->B->C
        let two_hop = paths.iter().find(|p| p.hops.len() == 2);
        assert!(two_hop.is_some());
        let two_hop = two_hop.unwrap();
        assert_eq!(two_hop.hops[0], ("A".into(), "knows".into(), "B".into()));
        assert_eq!(two_hop.hops[1], ("B".into(), "knows".into(), "C".into()));
    }

    #[test]
    fn find_paths_between_entities() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("A", "knows", "B"));
        graph.upsert_triple(&make_triple("B", "knows", "C"));

        let paths = graph.find_paths("A", "C", 3);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].hops.len(), 2);
    }

    #[test]
    fn find_paths_returns_empty_when_no_connection() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("A", "knows", "B"));
        graph.upsert_triple(&make_triple("C", "knows", "D"));

        let paths = graph.find_paths("A", "D", 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn detect_communities_two_separate_subgraphs() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        // Subgraph 1: A-B-C
        graph.upsert_triple(&make_triple("A", "knows", "B"));
        graph.upsert_triple(&make_triple("B", "knows", "C"));

        // Subgraph 2: X-Y
        graph.upsert_triple(&make_triple("X", "likes", "Y"));

        let communities = graph.detect_communities();
        assert_eq!(communities.len(), 2);

        // One community should have 3 entities, the other 2
        let mut sizes: Vec<usize> = communities.iter().map(|c| c.entities.len()).collect();
        sizes.sort();
        assert_eq!(sizes, vec![2, 3]);
    }

    #[test]
    fn neighborhood_returns_correct_edges() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("A", "knows", "B"));
        graph.upsert_triple(&make_triple("C", "likes", "B"));
        graph.upsert_triple(&make_triple("B", "has", "D"));

        let nb = graph.neighborhood("B");
        assert_eq!(nb.entity, "B");
        // Outbound: B->D
        assert_eq!(nb.outbound.len(), 1);
        assert_eq!(nb.outbound[0].target, "D");
        // Inbound: A->B, C->B
        assert_eq!(nb.inbound.len(), 2);
        let inbound_sources: HashSet<&str> = nb.inbound.iter().map(|e| e.target.as_str()).collect();
        assert!(inbound_sources.contains("A"));
        assert!(inbound_sources.contains("C"));
    }

    #[test]
    fn search_entities_exact_match() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("Rust", "is_a", "language"));

        let results = graph.search_entities("Rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "Rust");
        assert!((results[0].1 - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn search_entities_substring_match() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("Rust programming", "is_a", "language"));

        let results = graph.search_entities("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "Rust programming");
        // Score should be 4/16 = 0.25
        assert!(results[0].1 < 1.0);
        assert!(results[0].1 > 0.0);
    }

    #[test]
    fn search_entities_no_match() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("Rust", "is_a", "language"));

        let results = graph.search_entities("Python");
        assert!(results.is_empty());
    }

    #[test]
    fn entity_and_edge_counts() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("A", "r1", "B"));
        graph.upsert_triple(&make_triple("B", "r2", "C"));
        graph.upsert_triple(&make_triple("A", "r3", "C"));

        assert_eq!(graph.entity_count(), 3);
        assert_eq!(graph.edge_count(), 3);
    }

    #[test]
    fn traverse_nonexistent_entity() {
        let graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };
        let paths = graph.traverse("nonexistent", 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn find_paths_nonexistent_entity() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };
        graph.upsert_triple(&make_triple("A", "knows", "B"));
        let paths = graph.find_paths("A", "nonexistent", 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn neighborhood_of_unknown_entity() {
        let graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };
        let nb = graph.neighborhood("unknown");
        assert!(nb.outbound.is_empty());
        assert!(nb.inbound.is_empty());
    }

    #[test]
    fn search_entities_case_insensitive() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("RUST", "is_a", "LANGUAGE"));

        let results = graph.search_entities("rust");
        assert_eq!(results.len(), 1);
        assert!((results[0].1 - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn community_edge_count() {
        let mut graph = KnowledgeGraph {
            adjacency: HashMap::new(),
            reverse: HashMap::new(),
            entities: HashSet::new(),
        };

        graph.upsert_triple(&make_triple("A", "r1", "B"));
        graph.upsert_triple(&make_triple("B", "r2", "A"));

        let communities = graph.detect_communities();
        assert_eq!(communities.len(), 1);
        assert_eq!(communities[0].edge_count, 2);
        assert_eq!(communities[0].entities.len(), 2);
    }
}
