//! Retrieval router for agentic RAG.
//!
//! Routes queries to the appropriate retrieval strategy (vector, keyword, or
//! graph) and executes retrieval with source attribution.

use aivyx_core::Result;

use crate::manager::MemoryManager;

/// Retrieval strategy for agentic RAG.
#[derive(Debug, Clone, PartialEq)]
pub enum RetrievalStrategy {
    /// Vector similarity search via embeddings.
    Vector,
    /// Keyword-based search via knowledge triples.
    Keyword,
    /// Graph traversal via KnowledgeGraph.
    Graph,
    /// Combined multi-source retrieval.
    MultiSource(Vec<RetrievalStrategy>),
}

/// Source attribution for a retrieval result.
#[derive(Debug, Clone)]
pub enum RetrievalSource {
    VectorMemory,
    KnowledgeTriple,
    GraphTraversal,
}

/// A single retrieval result with source attribution.
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub content: String,
    pub source: RetrievalSource,
    pub relevance: f32,
}

/// Attribution for a synthesized answer.
#[derive(Debug, Clone)]
pub struct Attribution {
    pub source: RetrievalSource,
    pub relevance: f32,
    pub supporting_claim: String,
}

/// Result of multi-source synthesis.
#[derive(Debug, Clone)]
pub struct SynthesisResult {
    pub answer: String,
    pub sources: Vec<Attribution>,
}

/// Routes queries to the appropriate retrieval strategy and executes retrieval.
pub struct RetrievalRouter;

impl RetrievalRouter {
    /// Classify a query into the best retrieval strategy.
    ///
    /// Uses simple heuristics (no LLM needed for v1):
    /// - If query mentions entities/relationships -> Graph
    /// - If query is a simple keyword/name lookup -> Keyword
    /// - Default -> Vector
    /// - If query is complex/multi-faceted -> MultiSource
    pub fn route(query: &str) -> RetrievalStrategy {
        let lower = query.to_lowercase();

        // Graph: relationship queries
        let graph_keywords = [
            "related to",
            "connected to",
            "relationship between",
            "how is",
            "path from",
            "path between",
            "link between",
        ];
        if graph_keywords.iter().any(|kw| lower.contains(kw)) {
            return RetrievalStrategy::Graph;
        }

        // Keyword: fact lookups
        let keyword_patterns = ["what is", "who is", "define ", "meaning of"];
        if keyword_patterns.iter().any(|kw| lower.contains(kw)) {
            return RetrievalStrategy::Keyword;
        }

        // MultiSource: complex queries with multiple aspects
        if lower.contains(" and ") && lower.len() > 80 {
            return RetrievalStrategy::MultiSource(vec![
                RetrievalStrategy::Vector,
                RetrievalStrategy::Keyword,
            ]);
        }

        // Default: vector similarity
        RetrievalStrategy::Vector
    }

    /// Execute retrieval using the given strategy.
    pub fn retrieve<'a>(
        manager: &'a mut MemoryManager,
        query: &'a str,
        strategy: &'a RetrievalStrategy,
        top_k: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<RetrievalResult>>> + Send + 'a>,
    > {
        Box::pin(async move {
            match strategy {
                RetrievalStrategy::Vector => {
                    let memories = manager.recall(query, top_k, None, &[]).await?;
                    Ok(memories
                        .into_iter()
                        .map(|m| RetrievalResult {
                            content: m.content,
                            source: RetrievalSource::VectorMemory,
                            relevance: 1.0, // recall already sorts by relevance
                        })
                        .collect())
                }
                RetrievalStrategy::Keyword => {
                    // Extract potential entity from query
                    let words: Vec<&str> = query.split_whitespace().collect();
                    let entity = words.last().unwrap_or(&"");
                    let triples = manager.query_triples(Some(entity), None, None, None)?;
                    let mut results: Vec<RetrievalResult> = triples
                        .into_iter()
                        .map(|t| RetrievalResult {
                            content: format!("{} {} {}", t.subject, t.predicate, t.object),
                            source: RetrievalSource::KnowledgeTriple,
                            relevance: t.confidence,
                        })
                        .collect();
                    // Also try as object
                    let obj_triples = manager.query_triples(None, None, Some(entity), None)?;
                    results.extend(obj_triples.into_iter().map(|t| RetrievalResult {
                        content: format!("{} {} {}", t.subject, t.predicate, t.object),
                        source: RetrievalSource::KnowledgeTriple,
                        relevance: t.confidence,
                    }));
                    results.truncate(top_k);
                    Ok(results)
                }
                RetrievalStrategy::Graph => {
                    if let Some(graph) = manager.graph() {
                        // Extract entities from query via graph search
                        let entities = graph.search_entities(query);
                        let mut results = Vec::new();
                        for (entity, score) in entities.iter().take(3) {
                            let paths = graph.traverse(entity, 2);
                            for path in paths.iter().take(top_k) {
                                let content = path
                                    .hops
                                    .iter()
                                    .map(|(s, p, o)| format!("{s} {p} {o}"))
                                    .collect::<Vec<_>>()
                                    .join(" -> ");
                                results.push(RetrievalResult {
                                    content,
                                    source: RetrievalSource::GraphTraversal,
                                    relevance: *score,
                                });
                            }
                        }
                        results.truncate(top_k);
                        Ok(results)
                    } else {
                        // Fall back to vector if no graph available
                        Self::retrieve(manager, query, &RetrievalStrategy::Vector, top_k).await
                    }
                }
                RetrievalStrategy::MultiSource(strategies) => {
                    let mut all_results = Vec::new();
                    for strat in strategies {
                        let results = Self::retrieve(manager, query, strat, top_k).await?;
                        all_results.extend(results);
                    }
                    // Sort by relevance descending, take top_k
                    all_results.sort_by(|a, b| {
                        b.relevance
                            .partial_cmp(&a.relevance)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    all_results.truncate(top_k);
                    Ok(all_results)
                }
            }
        })
    }

    /// Evaluate the relevance of retrieval results (simple threshold filter).
    pub fn filter_by_relevance(
        results: Vec<RetrievalResult>,
        min_relevance: f32,
    ) -> Vec<RetrievalResult> {
        results
            .into_iter()
            .filter(|r| r.relevance >= min_relevance)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_classifies_graph_queries() {
        assert_eq!(
            RetrievalRouter::route("How is Rust related to LLVM?"),
            RetrievalStrategy::Graph
        );
        assert_eq!(
            RetrievalRouter::route("path between A and B"),
            RetrievalStrategy::Graph
        );
        assert_eq!(
            RetrievalRouter::route("What is connected to the database?"),
            RetrievalStrategy::Graph
        );
        assert_eq!(
            RetrievalRouter::route("relationship between X and Y"),
            RetrievalStrategy::Graph
        );
    }

    #[test]
    fn route_classifies_keyword_queries() {
        assert_eq!(
            RetrievalRouter::route("What is Rust?"),
            RetrievalStrategy::Keyword
        );
        assert_eq!(
            RetrievalRouter::route("Who is the author?"),
            RetrievalStrategy::Keyword
        );
        assert_eq!(
            RetrievalRouter::route("Define polymorphism"),
            RetrievalStrategy::Keyword
        );
        assert_eq!(
            RetrievalRouter::route("meaning of life"),
            RetrievalStrategy::Keyword
        );
    }

    #[test]
    fn route_classifies_complex_queries_as_multisource() {
        // Must contain " and " and be > 80 chars
        let long_query = "Explain the architecture of the memory subsystem and how it integrates with the knowledge graph for retrieval";
        assert!(long_query.len() > 80);
        let strategy = RetrievalRouter::route(long_query);
        assert_eq!(
            strategy,
            RetrievalStrategy::MultiSource(vec![
                RetrievalStrategy::Vector,
                RetrievalStrategy::Keyword,
            ])
        );
    }

    #[test]
    fn route_defaults_to_vector() {
        assert_eq!(
            RetrievalRouter::route("Tell me about Rust performance"),
            RetrievalStrategy::Vector
        );
        assert_eq!(
            RetrievalRouter::route("summarize recent sessions"),
            RetrievalStrategy::Vector
        );
    }

    #[test]
    fn filter_by_relevance_filters_correctly() {
        let results = vec![
            RetrievalResult {
                content: "high".into(),
                source: RetrievalSource::VectorMemory,
                relevance: 0.9,
            },
            RetrievalResult {
                content: "low".into(),
                source: RetrievalSource::KnowledgeTriple,
                relevance: 0.3,
            },
            RetrievalResult {
                content: "medium".into(),
                source: RetrievalSource::GraphTraversal,
                relevance: 0.6,
            },
        ];

        let filtered = RetrievalRouter::filter_by_relevance(results, 0.5);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].content, "high");
        assert_eq!(filtered[1].content, "medium");
    }

    #[test]
    fn retrieval_strategy_equality() {
        assert_eq!(RetrievalStrategy::Vector, RetrievalStrategy::Vector);
        assert_eq!(RetrievalStrategy::Keyword, RetrievalStrategy::Keyword);
        assert_eq!(RetrievalStrategy::Graph, RetrievalStrategy::Graph);
        assert_ne!(RetrievalStrategy::Vector, RetrievalStrategy::Keyword);
        assert_ne!(RetrievalStrategy::Vector, RetrievalStrategy::Graph);

        let ms1 = RetrievalStrategy::MultiSource(vec![
            RetrievalStrategy::Vector,
            RetrievalStrategy::Keyword,
        ]);
        let ms2 = RetrievalStrategy::MultiSource(vec![
            RetrievalStrategy::Vector,
            RetrievalStrategy::Keyword,
        ]);
        assert_eq!(ms1, ms2);

        let ms3 = RetrievalStrategy::MultiSource(vec![RetrievalStrategy::Vector]);
        assert_ne!(ms1, ms3);
    }

    #[test]
    fn retrieval_result_fields_accessible() {
        let result = RetrievalResult {
            content: "test content".into(),
            source: RetrievalSource::VectorMemory,
            relevance: 0.85,
        };
        assert_eq!(result.content, "test content");
        assert!((result.relevance - 0.85).abs() < f32::EPSILON);
        assert!(matches!(result.source, RetrievalSource::VectorMemory));

        let result2 = RetrievalResult {
            content: "triple".into(),
            source: RetrievalSource::KnowledgeTriple,
            relevance: 0.7,
        };
        assert!(matches!(result2.source, RetrievalSource::KnowledgeTriple));

        let result3 = RetrievalResult {
            content: "graph".into(),
            source: RetrievalSource::GraphTraversal,
            relevance: 0.5,
        };
        assert!(matches!(result3.source, RetrievalSource::GraphTraversal));
    }
}
