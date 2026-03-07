//! Memory tools that agents call during conversation.
//!
//! Three tools:
//! - `memory_store` — store a new memory
//! - `memory_search` — search memories by semantic similarity
//! - `memory_triple` — add or query knowledge triples

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use aivyx_core::{AgentId, AivyxError, CapabilityScope, Result, Tool, ToolId};

use crate::manager::MemoryManager;
use crate::types::MemoryKind;

// ---------------------------------------------------------------------------
// memory_store
// ---------------------------------------------------------------------------

/// Tool for storing a memory.
pub struct MemoryStoreTool {
    id: ToolId,
    manager: Arc<Mutex<MemoryManager>>,
    agent_id: AgentId,
}

impl MemoryStoreTool {
    pub fn new(manager: Arc<Mutex<MemoryManager>>, agent_id: AgentId) -> Self {
        Self {
            id: ToolId::new(),
            manager,
            agent_id,
        }
    }
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store a memory for later retrieval. Use this to remember facts, preferences, procedures, or session summaries."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The text content to remember"
                },
                "kind": {
                    "type": "string",
                    "enum": ["fact", "preference", "session_summary", "procedure"],
                    "description": "The kind of memory"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags for categorization"
                }
            },
            "required": ["content", "kind"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("memory".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("memory_store: missing 'content'".into()))?
            .to_string();

        let kind = match input["kind"].as_str() {
            Some("fact") => MemoryKind::Fact,
            Some("preference") => MemoryKind::Preference,
            Some("session_summary") => MemoryKind::SessionSummary,
            Some("procedure") => MemoryKind::Procedure,
            Some("decision") => MemoryKind::Decision,
            Some("outcome") => MemoryKind::Outcome,
            Some(other) => MemoryKind::Custom(other.to_string()),
            None => return Err(AivyxError::Agent("memory_store: missing 'kind'".into())),
        };

        let tags: Vec<String> = input["tags"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut mgr = self.manager.lock().await;
        let id = mgr
            .remember(content, kind, Some(self.agent_id), tags)
            .await?;

        Ok(serde_json::json!({
            "status": "stored",
            "memory_id": id.to_string(),
        }))
    }
}

// ---------------------------------------------------------------------------
// memory_search
// ---------------------------------------------------------------------------

/// Tool for searching memories by semantic similarity.
pub struct MemorySearchTool {
    id: ToolId,
    manager: Arc<Mutex<MemoryManager>>,
    agent_id: AgentId,
}

impl MemorySearchTool {
    pub fn new(manager: Arc<Mutex<MemoryManager>>, agent_id: AgentId) -> Self {
        Self {
            id: ToolId::new(),
            manager,
            agent_id,
        }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search for relevant memories using semantic similarity. Returns the most relevant stored memories."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5)"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Filter results to only memories containing ALL of these tags (e.g., [\"project:aivyx\"])"
                }
            },
            "required": ["query"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("memory".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("memory_search: missing 'query'".into()))?;

        let top_k = input["top_k"].as_u64().unwrap_or(5) as usize;

        let required_tags: Vec<String> = input["tags"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut mgr = self.manager.lock().await;
        let entries = mgr
            .recall(query, top_k, Some(self.agent_id), &required_tags)
            .await?;

        let results: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "memory_id": e.id.to_string(),
                    "content": e.content,
                    "kind": format!("{:?}", e.kind),
                    "tags": e.tags,
                    "access_count": e.access_count,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "results": results,
            "count": results.len(),
        }))
    }
}

// ---------------------------------------------------------------------------
// memory_triple
// ---------------------------------------------------------------------------

/// Tool for adding or querying knowledge triples.
pub struct MemoryTripleTool {
    id: ToolId,
    manager: Arc<Mutex<MemoryManager>>,
    agent_id: AgentId,
}

impl MemoryTripleTool {
    pub fn new(manager: Arc<Mutex<MemoryManager>>, agent_id: AgentId) -> Self {
        Self {
            id: ToolId::new(),
            manager,
            agent_id,
        }
    }
}

#[async_trait]
impl Tool for MemoryTripleTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "memory_triple"
    }

    fn description(&self) -> &str {
        "Add or query knowledge triples (subject-predicate-object facts)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "query"],
                    "description": "Whether to add a new triple or query existing ones"
                },
                "subject": {
                    "type": "string",
                    "description": "The subject entity"
                },
                "predicate": {
                    "type": "string",
                    "description": "The relationship"
                },
                "object": {
                    "type": "string",
                    "description": "The object entity"
                },
                "confidence": {
                    "type": "number",
                    "description": "Confidence score 0.0-1.0 (for add, default: 0.9)"
                }
            },
            "required": ["action"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("memory".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("memory_triple: missing 'action'".into()))?;

        let mut mgr = self.manager.lock().await;

        match action {
            "add" => {
                let subject = input["subject"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("memory_triple: missing 'subject'".into()))?
                    .to_string();
                let predicate = input["predicate"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("memory_triple: missing 'predicate'".into()))?
                    .to_string();
                let object = input["object"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("memory_triple: missing 'object'".into()))?
                    .to_string();
                let confidence = input["confidence"].as_f64().unwrap_or(0.9) as f32;

                let id = mgr.add_triple(
                    subject,
                    predicate,
                    object,
                    Some(self.agent_id),
                    confidence,
                    "agent".into(),
                )?;

                Ok(serde_json::json!({
                    "status": "added",
                    "triple_id": id.to_string(),
                }))
            }
            "query" => {
                let subject = input["subject"].as_str();
                let predicate = input["predicate"].as_str();
                let object = input["object"].as_str();

                let triples = mgr.query_triples(subject, predicate, object, Some(self.agent_id))?;

                let results: Vec<serde_json::Value> = triples
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "triple_id": t.id.to_string(),
                            "subject": t.subject,
                            "predicate": t.predicate,
                            "object": t.object,
                            "confidence": t.confidence,
                        })
                    })
                    .collect();

                Ok(serde_json::json!({
                    "results": results,
                    "count": results.len(),
                }))
            }
            other => Err(AivyxError::Agent(format!(
                "memory_triple: unknown action '{other}', expected 'add' or 'query'"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// memory_retrieve
// ---------------------------------------------------------------------------

/// Tool for agentic RAG retrieval with automatic strategy routing.
pub struct MemoryRetrieveTool {
    id: ToolId,
    manager: Arc<Mutex<MemoryManager>>,
}

impl MemoryRetrieveTool {
    pub fn new(manager: Arc<Mutex<MemoryManager>>) -> Self {
        Self {
            id: ToolId::new(),
            manager,
        }
    }
}

#[async_trait]
impl Tool for MemoryRetrieveTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "memory_retrieve"
    }

    fn description(&self) -> &str {
        "Retrieve relevant information using agentic RAG with automatic strategy routing. Routes queries to vector, keyword, or graph retrieval based on query analysis."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The query to retrieve information for"
                },
                "strategy": {
                    "type": "string",
                    "enum": ["auto", "vector", "keyword", "graph"],
                    "description": "Retrieval strategy to use (default: auto)"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("memory".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        use crate::retrieval::{RetrievalRouter, RetrievalStrategy};

        let query = input["query"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("memory_retrieve: missing 'query'".into()))?;

        let top_k = input["top_k"].as_u64().unwrap_or(5) as usize;

        let strategy_str = input["strategy"].as_str().unwrap_or("auto");
        let strategy = match strategy_str {
            "vector" => RetrievalStrategy::Vector,
            "keyword" => RetrievalStrategy::Keyword,
            "graph" => RetrievalStrategy::Graph,
            "auto" | _ => RetrievalRouter::route(query),
        };

        let strategy_name = match &strategy {
            RetrievalStrategy::Vector => "vector",
            RetrievalStrategy::Keyword => "keyword",
            RetrievalStrategy::Graph => "graph",
            RetrievalStrategy::MultiSource(_) => "multi_source",
        };

        let mut mgr = self.manager.lock().await;
        let results = RetrievalRouter::retrieve(&mut mgr, query, &strategy, top_k).await?;

        let result_values: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                let source_name = match &r.source {
                    crate::retrieval::RetrievalSource::VectorMemory => "vector_memory",
                    crate::retrieval::RetrievalSource::KnowledgeTriple => "knowledge_triple",
                    crate::retrieval::RetrievalSource::GraphTraversal => "graph_traversal",
                };
                serde_json::json!({
                    "content": r.content,
                    "source": source_name,
                    "relevance": r.relevance,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "results": result_values,
            "count": result_values.len(),
            "strategy": strategy_name,
        }))
    }
}

/// Register all memory tools into a `ToolRegistry`.
pub fn register_memory_tools(
    registry: &mut aivyx_core::ToolRegistry,
    manager: Arc<Mutex<MemoryManager>>,
    agent_id: AgentId,
) {
    registry.register(Box::new(MemoryStoreTool::new(
        Arc::clone(&manager),
        agent_id,
    )));
    registry.register(Box::new(MemorySearchTool::new(
        Arc::clone(&manager),
        agent_id,
    )));
    registry.register(Box::new(MemoryTripleTool::new(
        Arc::clone(&manager),
        agent_id,
    )));
    registry.register(Box::new(MemoryRetrieveTool::new(manager)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool_set() -> (
        MemoryStoreTool,
        MemorySearchTool,
        MemoryTripleTool,
        MemoryRetrieveTool,
        std::path::PathBuf,
    ) {
        use crate::store::MemoryStore;
        use aivyx_crypto::MasterKey;
        use async_trait::async_trait;

        struct DummyProvider;

        #[async_trait]
        impl aivyx_llm::EmbeddingProvider for DummyProvider {
            fn name(&self) -> &str {
                "dummy"
            }
            fn dimensions(&self) -> usize {
                4
            }
            async fn embed(&self, _text: &str) -> Result<aivyx_llm::Embedding> {
                Ok(aivyx_llm::Embedding {
                    vector: vec![0.1, 0.2, 0.3, 0.4],
                    dimensions: 4,
                })
            }
        }

        let dir = std::env::temp_dir().join(format!("aivyx-tools-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db");
        let store = MemoryStore::open(&db_path).unwrap();
        let master_key = MasterKey::generate();
        let provider: Arc<dyn aivyx_llm::EmbeddingProvider> = Arc::new(DummyProvider);
        let mgr = MemoryManager::new(store, provider, master_key, 0).unwrap();
        let manager = Arc::new(Mutex::new(mgr));
        let agent_id = AgentId::new();

        let store_tool = MemoryStoreTool::new(Arc::clone(&manager), agent_id);
        let search_tool = MemorySearchTool::new(Arc::clone(&manager), agent_id);
        let triple_tool = MemoryTripleTool::new(Arc::clone(&manager), agent_id);
        let retrieve_tool = MemoryRetrieveTool::new(manager);

        (store_tool, search_tool, triple_tool, retrieve_tool, dir)
    }

    #[test]
    fn store_tool_schema() {
        let (tool, _, _, _, dir) = make_tool_set();
        assert_eq!(tool.name(), "memory_store");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["content"].is_object());
        assert!(schema["properties"]["kind"].is_object());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_tool_schema() {
        let (_, tool, _, _, dir) = make_tool_set();
        assert_eq!(tool.name(), "memory_search");
        let schema = tool.input_schema();
        assert!(schema["properties"]["query"].is_object());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn triple_tool_schema() {
        let (_, _, tool, _, dir) = make_tool_set();
        assert_eq!(tool.name(), "memory_triple");
        let schema = tool.input_schema();
        assert!(schema["properties"]["action"].is_object());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_tool_required_scope() {
        let (tool, _, _, _, dir) = make_tool_set();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref name) if name == "memory"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_tool_required_scope() {
        let (_, tool, _, _, dir) = make_tool_set();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref name) if name == "memory"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn triple_tool_required_scope() {
        let (_, _, tool, _, dir) = make_tool_set();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref name) if name == "memory"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tool_names_unique() {
        let (s, m, t, r, dir) = make_tool_set();
        let names = [s.name(), m.name(), t.name(), r.name()];
        let mut sorted = names.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retrieve_tool_schema() {
        let (_, _, _, tool, dir) = make_tool_set();
        assert_eq!(tool.name(), "memory_retrieve");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["strategy"].is_object());
        assert!(schema["properties"]["top_k"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "query");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retrieve_tool_required_scope() {
        let (_, _, _, tool, dir) = make_tool_set();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref name) if name == "memory"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn register_adds_all_four() {
        use crate::store::MemoryStore;
        use aivyx_crypto::MasterKey;
        use async_trait::async_trait;

        struct DummyProvider;

        #[async_trait]
        impl aivyx_llm::EmbeddingProvider for DummyProvider {
            fn name(&self) -> &str {
                "dummy"
            }
            fn dimensions(&self) -> usize {
                4
            }
            async fn embed(&self, _text: &str) -> Result<aivyx_llm::Embedding> {
                Ok(aivyx_llm::Embedding {
                    vector: vec![0.0; 4],
                    dimensions: 4,
                })
            }
        }

        let dir =
            std::env::temp_dir().join(format!("aivyx-tools-reg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = MemoryStore::open(dir.join("memory.db")).unwrap();
        let master_key = MasterKey::generate();
        let provider: Arc<dyn aivyx_llm::EmbeddingProvider> = Arc::new(DummyProvider);
        let mgr = MemoryManager::new(store, provider, master_key, 0).unwrap();
        let manager = Arc::new(Mutex::new(mgr));

        let mut registry = aivyx_core::ToolRegistry::new();
        register_memory_tools(&mut registry, manager, AgentId::new());

        assert_eq!(registry.list().len(), 4);
        assert!(registry.get_by_name("memory_store").is_some());
        assert!(registry.get_by_name("memory_search").is_some());
        assert!(registry.get_by_name("memory_triple").is_some());
        assert!(registry.get_by_name("memory_retrieve").is_some());

        std::fs::remove_dir_all(&dir).ok();
    }
}
