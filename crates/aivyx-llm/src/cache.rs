//! LLM response caching with exact-match and semantic similarity.
//!
//! Provides two caching layers to reduce redundant LLM API calls:
//!
//! - **Prompt cache**: SHA-256 hash of the full request for O(1) exact match.
//! - **Semantic cache**: Cosine similarity on the last user message embedding,
//!   scoped by system prompt to prevent cross-persona contamination.
//!
//! The [`CachingProvider`] wraps any [`LlmProvider`] and intercepts `chat()`
//! calls, checking both caches before delegating to the inner provider.
//! Implements [`LlmProvider`] itself, making caching transparent to agents.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::debug;

use aivyx_config::CacheConfig;
use aivyx_core::Result;

use crate::embedding::EmbeddingProvider;
use crate::message::{ChatRequest, ChatResponse, Role, StopReason};
use crate::provider::{LlmProvider, StreamEvent};

// ---------------------------------------------------------------------------
// Cache events & observer
// ---------------------------------------------------------------------------

/// Events emitted by the caching layer for observability.
#[derive(Debug, Clone)]
pub enum CacheEvent {
    /// An exact-match prompt cache hit.
    PromptCacheHit {
        prompt_hash: String,
        tokens_saved: u32,
    },
    /// A semantic similarity cache hit.
    SemanticCacheHit { similarity: f32, tokens_saved: u32 },
}

/// Observer callback for cache events (bridges to audit log without
/// creating a direct dependency from aivyx-llm to aivyx-audit).
pub type CacheObserver = Arc<dyn Fn(CacheEvent) + Send + Sync>;

// ---------------------------------------------------------------------------
// Cache metrics
// ---------------------------------------------------------------------------

/// Atomic counters for cache performance tracking.
pub struct CacheMetrics {
    pub prompt_hits: AtomicU64,
    pub prompt_misses: AtomicU64,
    pub semantic_hits: AtomicU64,
    pub semantic_misses: AtomicU64,
    /// Estimated input tokens saved across all cache hits.
    pub tokens_saved: AtomicU64,
}

impl CacheMetrics {
    fn new() -> Self {
        Self {
            prompt_hits: AtomicU64::new(0),
            prompt_misses: AtomicU64::new(0),
            semantic_hits: AtomicU64::new(0),
            semantic_misses: AtomicU64::new(0),
            tokens_saved: AtomicU64::new(0),
        }
    }

    fn record_prompt_hit(&self, tokens: u32) {
        self.prompt_hits.fetch_add(1, Ordering::Relaxed);
        self.tokens_saved
            .fetch_add(u64::from(tokens), Ordering::Relaxed);
    }

    fn record_prompt_miss(&self) {
        self.prompt_misses.fetch_add(1, Ordering::Relaxed);
    }

    fn record_semantic_hit(&self, tokens: u32) {
        self.semantic_hits.fetch_add(1, Ordering::Relaxed);
        self.tokens_saved
            .fetch_add(u64::from(tokens), Ordering::Relaxed);
    }

    fn record_semantic_miss(&self) {
        self.semantic_misses.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Prompt cache (exact match)
// ---------------------------------------------------------------------------

struct PromptCacheEntry {
    response: ChatResponse,
    expires_at: Instant,
}

/// In-memory exact-match cache keyed by SHA-256 of the full request.
struct PromptCache {
    entries: Mutex<HashMap<String, PromptCacheEntry>>,
    ttl: Duration,
    max_entries: usize,
}

impl PromptCache {
    fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    fn get(&self, key: &str) -> Option<ChatResponse> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get(key) {
            if entry.expires_at > Instant::now() {
                return Some(entry.response.clone());
            }
            entries.remove(key);
        }
        None
    }

    fn insert(&self, key: &str, response: ChatResponse) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        if entries.len() >= self.max_entries && !entries.contains_key(key) {
            let now = Instant::now();
            entries.retain(|_, e| e.expires_at > now);

            if entries.len() >= self.max_entries
                && let Some(oldest_key) = entries
                    .iter()
                    .min_by_key(|(_, e)| e.expires_at)
                    .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest_key);
            }
        }

        entries.insert(
            key.to_string(),
            PromptCacheEntry {
                response,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

// ---------------------------------------------------------------------------
// Semantic cache (embedding similarity)
// ---------------------------------------------------------------------------

struct SemanticCacheEntry {
    /// Hash of the system prompt for scope isolation.
    system_prompt_hash: String,
    /// Embedding vector of the last user message.
    embedding: Vec<f32>,
    /// The cached response.
    response: ChatResponse,
    /// When this entry expires.
    expires_at: Instant,
}

/// In-memory cache using cosine similarity on message embeddings.
struct SemanticCache {
    entries: Mutex<Vec<SemanticCacheEntry>>,
    ttl: Duration,
    max_entries: usize,
    similarity_threshold: f32,
}

impl SemanticCache {
    fn new(ttl: Duration, max_entries: usize, similarity_threshold: f32) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            ttl,
            max_entries,
            similarity_threshold,
        }
    }

    /// Find the best matching cached response above the similarity threshold.
    ///
    /// Only entries with a matching `system_prompt_hash` are considered,
    /// ensuring agents with different personas never share cached responses.
    fn find(&self, sys_hash: &str, query_embedding: &[f32]) -> Option<(ChatResponse, f32)> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        // Evict expired during scan.
        entries.retain(|e| e.expires_at > now);

        let mut best: Option<(usize, f32)> = None;

        for (i, entry) in entries.iter().enumerate() {
            if entry.system_prompt_hash != sys_hash {
                continue;
            }
            let sim = cosine_similarity(query_embedding, &entry.embedding);
            if sim >= self.similarity_threshold && (best.is_none() || sim > best.unwrap().1) {
                best = Some((i, sim));
            }
        }

        best.map(|(i, sim)| (entries[i].response.clone(), sim))
    }

    fn insert(&self, system_prompt_hash: String, embedding: Vec<f32>, response: ChatResponse) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        // Evict expired.
        let now = Instant::now();
        entries.retain(|e| e.expires_at > now);

        // If at capacity, remove oldest.
        if entries.len() >= self.max_entries
            && let Some(oldest_idx) = entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.expires_at)
                .map(|(i, _)| i)
        {
            entries.remove(oldest_idx);
        }

        entries.push(SemanticCacheEntry {
            system_prompt_hash,
            embedding,
            response,
            expires_at: now + self.ttl,
        });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// Cosine similarity between two vectors.
///
/// Returns 0.0 for zero-magnitude vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        0.0
    } else {
        dot / (mag_a * mag_b)
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Generate a deterministic cache key from a chat request.
///
/// Hashes: system_prompt + serialized messages + model + max_tokens.
/// Tools are intentionally excluded — the same prompt with different
/// available tools should produce a cache hit.
fn prompt_cache_key(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();
    if let Some(ref sp) = request.system_prompt {
        hasher.update(sp.as_bytes());
    }
    hasher.update(b"|");
    for msg in &request.messages {
        // Serialize role + content deterministically.
        let role_byte = match msg.role {
            Role::System => b'S',
            Role::User => b'U',
            Role::Assistant => b'A',
            Role::Tool => b'T',
        };
        hasher.update([role_byte]);
        hasher.update(msg.content.to_text().as_bytes());
        // Include tool calls and results in the key.
        for tc in &msg.tool_calls {
            hasher.update(tc.id.as_bytes());
            hasher.update(tc.name.as_bytes());
            hasher.update(tc.arguments.to_string().as_bytes());
        }
        if let Some(ref tr) = msg.tool_result {
            hasher.update(tr.tool_call_id.as_bytes());
            hasher.update(tr.content.to_string().as_bytes());
        }
    }
    hasher.update(b"|");
    if let Some(ref model) = request.model {
        hasher.update(model.as_bytes());
    }
    hasher.update(b"|");
    hasher.update(request.max_tokens.to_le_bytes());
    hex::encode(hasher.finalize())
}

/// Hash the system prompt for semantic cache scope isolation.
fn system_prompt_hash(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();
    if let Some(ref sp) = request.system_prompt {
        hasher.update(sp.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Extract the text content of the last user message in a request.
fn extract_last_user_message(request: &ChatRequest) -> Option<&str> {
    request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.text())
}

// ---------------------------------------------------------------------------
// CachingProvider
// ---------------------------------------------------------------------------

/// LLM provider wrapper that adds prompt and semantic caching.
///
/// Intercepts `chat()` calls to check both caches before delegating to
/// the inner provider. Implements [`LlmProvider`] so it is transparent
/// to the agent — the agent sees a single provider that happens to cache.
pub struct CachingProvider {
    inner: Box<dyn LlmProvider>,
    prompt_cache: PromptCache,
    semantic_cache: Option<SemanticCache>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    metrics: Arc<CacheMetrics>,
    observer: Option<CacheObserver>,
}

impl CachingProvider {
    /// Create a caching provider wrapping an inner provider.
    pub fn new(inner: Box<dyn LlmProvider>, config: &CacheConfig) -> Self {
        let prompt_cache =
            PromptCache::new(Duration::from_secs(config.ttl_secs), config.max_entries);

        let semantic_cache = if config.semantic_enabled {
            Some(SemanticCache::new(
                Duration::from_secs(config.ttl_secs),
                config.semantic_max_entries,
                config.similarity_threshold,
            ))
        } else {
            None
        };

        Self {
            inner,
            prompt_cache,
            semantic_cache,
            embedding_provider: None,
            metrics: Arc::new(CacheMetrics::new()),
            observer: None,
        }
    }

    /// Attach an embedding provider for semantic caching.
    pub fn with_semantic(mut self, embedding_provider: Arc<dyn EmbeddingProvider>) -> Self {
        self.embedding_provider = Some(embedding_provider);
        self
    }

    /// Attach an observer for cache events.
    pub fn with_observer(mut self, observer: CacheObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Get a reference to the cache metrics.
    pub fn metrics(&self) -> &Arc<CacheMetrics> {
        &self.metrics
    }

    fn emit(&self, event: CacheEvent) {
        if let Some(ref obs) = self.observer {
            obs(event);
        }
    }
}

#[async_trait]
impl LlmProvider for CachingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn context_window(&self) -> u32 {
        self.inner.context_window()
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        // 1. Check prompt cache (O(1) hash lookup).
        let prompt_key = prompt_cache_key(request);
        if let Some(cached) = self.prompt_cache.get(&prompt_key) {
            let tokens = cached.usage.input_tokens;
            self.metrics.record_prompt_hit(tokens);
            self.emit(CacheEvent::PromptCacheHit {
                prompt_hash: prompt_key,
                tokens_saved: tokens,
            });
            debug!("Prompt cache hit (saved ~{tokens} input tokens)");
            return Ok(cached);
        }

        // 2. Check semantic cache (O(n), only if configured).
        let mut computed_embedding: Option<Vec<f32>> = None;
        if let (Some(sc), Some(emb)) = (&self.semantic_cache, &self.embedding_provider)
            && let Some(last_msg) = extract_last_user_message(request)
        {
            let sys_hash = system_prompt_hash(request);
            match emb.embed(last_msg).await {
                Ok(embedding) => {
                    if let Some((cached, similarity)) = sc.find(&sys_hash, &embedding.vector) {
                        let tokens = cached.usage.input_tokens;
                        self.metrics.record_semantic_hit(tokens);
                        self.emit(CacheEvent::SemanticCacheHit {
                            similarity,
                            tokens_saved: tokens,
                        });
                        debug!(
                            "Semantic cache hit (similarity={similarity:.3}, \
                             saved ~{tokens} input tokens)"
                        );
                        return Ok(cached);
                    }
                    // Store embedding for insertion after provider call.
                    computed_embedding = Some(embedding.vector);
                }
                Err(e) => {
                    tracing::warn!("Semantic cache embedding failed: {e}, skipping");
                }
            }
        }

        self.metrics.record_prompt_miss();
        if self.semantic_cache.is_some() {
            self.metrics.record_semantic_miss();
        }

        // 3. Call inner provider.
        let response = self.inner.chat(request).await?;

        // 4. Cache response (unless it triggered tool use).
        if response.stop_reason != StopReason::ToolUse {
            self.prompt_cache.insert(&prompt_key, response.clone());

            // Insert into semantic cache using the pre-computed embedding.
            if let (Some(sc), Some(embedding)) = (&self.semantic_cache, computed_embedding) {
                let sys_hash = system_prompt_hash(request);
                sc.insert(sys_hash, embedding, response.clone());
            }
        }

        Ok(response)
    }

    async fn chat_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        // Streaming bypasses cache — interactive responses are expected
        // to be generated in real-time.
        self.inner.chat_stream(request, tx).await
    }

    async fn health_check(&self) -> Result<()> {
        self.inner.health_check().await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ChatMessage, TokenUsage};
    use std::sync::Mutex as StdMutex;

    // ---- Helper: mock provider ----

    struct MockProvider {
        name: String,
        calls: StdMutex<u32>,
        response: ChatResponse,
    }

    impl MockProvider {
        fn new(name: &str, response: ChatResponse) -> Box<Self> {
            Box::new(Self {
                name: name.into(),
                calls: StdMutex::new(0),
                response,
            })
        }

        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
            *self.calls.lock().unwrap() += 1;
            Ok(self.response.clone())
        }
    }

    fn test_response() -> ChatResponse {
        ChatResponse {
            message: ChatMessage::assistant("cached answer"),
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
            },
            stop_reason: StopReason::EndTurn,
        }
    }

    fn tool_use_response() -> ChatResponse {
        ChatResponse {
            message: ChatMessage::assistant("calling tool"),
            usage: TokenUsage {
                input_tokens: 80,
                output_tokens: 20,
            },
            stop_reason: StopReason::ToolUse,
        }
    }

    fn test_request() -> ChatRequest {
        ChatRequest {
            system_prompt: Some("You are helpful.".into()),
            messages: vec![ChatMessage::user("What is 2+2?")],
            tools: vec![],
            model: None,
            max_tokens: 100,
        }
    }

    fn test_config() -> CacheConfig {
        CacheConfig {
            enabled: true,
            ttl_secs: 3600,
            max_entries: 512,
            semantic_enabled: false,
            similarity_threshold: 0.95,
            semantic_max_entries: 256,
        }
    }

    // ---- Prompt cache tests ----

    #[test]
    fn prompt_cache_insert_and_get() {
        let cache = PromptCache::new(Duration::from_secs(60), 100);
        let key = "test-key";
        let resp = test_response();

        assert!(cache.get(key).is_none());
        cache.insert(key, resp.clone());
        let cached = cache.get(key).unwrap();
        assert_eq!(cached.message.content.to_text(), "cached answer");
    }

    #[test]
    fn prompt_cache_expired_returns_none() {
        let cache = PromptCache::new(Duration::from_millis(1), 100);
        cache.insert("key", test_response());
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get("key").is_none());
    }

    #[test]
    fn prompt_cache_evicts_at_capacity() {
        let cache = PromptCache::new(Duration::from_secs(300), 2);
        cache.insert("a", test_response());
        cache.insert("b", test_response());
        assert_eq!(cache.len(), 2);

        cache.insert("c", test_response());
        assert_eq!(cache.len(), 2);
        assert!(cache.get("c").is_some());
        assert!(cache.get("b").is_some());
        assert!(cache.get("a").is_none());
    }

    #[test]
    fn prompt_cache_key_deterministic() {
        let req = test_request();
        let k1 = prompt_cache_key(&req);
        let k2 = prompt_cache_key(&req);
        assert_eq!(k1, k2);
    }

    #[test]
    fn prompt_cache_key_differs_by_model() {
        let mut r1 = test_request();
        let mut r2 = test_request();
        r1.model = Some("gpt-4o".into());
        r2.model = Some("claude-sonnet".into());
        assert_ne!(prompt_cache_key(&r1), prompt_cache_key(&r2));
    }

    #[test]
    fn prompt_cache_key_differs_by_system_prompt() {
        let mut r1 = test_request();
        let mut r2 = test_request();
        r1.system_prompt = Some("Persona A".into());
        r2.system_prompt = Some("Persona B".into());
        assert_ne!(prompt_cache_key(&r1), prompt_cache_key(&r2));
    }

    #[test]
    fn prompt_cache_key_excludes_tools() {
        let mut r1 = test_request();
        let mut r2 = test_request();
        r1.tools = vec![serde_json::json!({"name": "search"})];
        r2.tools = vec![];
        assert_eq!(prompt_cache_key(&r1), prompt_cache_key(&r2));
    }

    #[test]
    fn prompt_cache_key_differs_by_messages() {
        let mut r1 = test_request();
        let mut r2 = test_request();
        r1.messages = vec![ChatMessage::user("Hello")];
        r2.messages = vec![ChatMessage::user("Goodbye")];
        assert_ne!(prompt_cache_key(&r1), prompt_cache_key(&r2));
    }

    // ---- Semantic cache tests ----

    #[test]
    fn semantic_cache_insert_and_find() {
        let cache = SemanticCache::new(Duration::from_secs(60), 100, 0.9);
        let embedding = vec![1.0, 0.0, 0.0];
        cache.insert("sys".into(), embedding.clone(), test_response());

        let result = cache.find("sys", &embedding);
        assert!(result.is_some());
        let (resp, sim) = result.unwrap();
        assert_eq!(resp.message.content.to_text(), "cached answer");
        assert!((sim - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn semantic_cache_below_threshold_returns_none() {
        let cache = SemanticCache::new(Duration::from_secs(60), 100, 0.95);
        cache.insert("sys".into(), vec![1.0, 0.0, 0.0], test_response());

        // Orthogonal vector — similarity = 0.0.
        let result = cache.find("sys", &[0.0, 1.0, 0.0]);
        assert!(result.is_none());
    }

    #[test]
    fn semantic_cache_respects_system_prompt_scope() {
        let cache = SemanticCache::new(Duration::from_secs(60), 100, 0.9);
        let embedding = vec![1.0, 0.0, 0.0];
        cache.insert("persona-a".into(), embedding.clone(), test_response());

        // Same embedding but different system prompt scope — should miss.
        let result = cache.find("persona-b", &embedding);
        assert!(result.is_none());
    }

    #[test]
    fn semantic_cache_expired_entries_cleaned() {
        let cache = SemanticCache::new(Duration::from_millis(1), 100, 0.9);
        cache.insert("sys".into(), vec![1.0, 0.0, 0.0], test_response());
        std::thread::sleep(Duration::from_millis(5));

        let result = cache.find("sys", &[1.0, 0.0, 0.0]);
        assert!(result.is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn semantic_cache_evicts_at_capacity() {
        let cache = SemanticCache::new(Duration::from_secs(300), 2, 0.9);
        cache.insert("sys".into(), vec![1.0, 0.0, 0.0], test_response());
        cache.insert("sys".into(), vec![0.0, 1.0, 0.0], test_response());
        assert_eq!(cache.len(), 2);

        cache.insert("sys".into(), vec![0.0, 0.0, 1.0], test_response());
        assert_eq!(cache.len(), 2);
    }

    // ---- Cosine similarity tests ----

    #[test]
    fn cosine_similarity_identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let sim = cosine_similarity(&[1.0, 0.0, 0.0], &[0.0, 1.0, 0.0]);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let sim = cosine_similarity(&[0.0, 0.0], &[1.0, 2.0]);
        assert!((sim - 0.0).abs() < f32::EPSILON);
    }

    // ---- CachingProvider tests ----

    #[tokio::test]
    async fn caching_provider_prompt_hit_returns_cached() {
        let mock = MockProvider::new("test", test_response());
        let mock_ref = mock.as_ref() as *const MockProvider;
        let provider = CachingProvider::new(mock, &test_config());

        let req = test_request();

        // First call — miss, delegates to inner.
        provider.chat(&req).await.unwrap();
        assert_eq!(unsafe { &*mock_ref }.call_count(), 1);

        // Second call — hit, returns cached.
        let cached = provider.chat(&req).await.unwrap();
        assert_eq!(cached.message.content.to_text(), "cached answer");
        assert_eq!(unsafe { &*mock_ref }.call_count(), 1); // not called again
    }

    #[tokio::test]
    async fn caching_provider_miss_calls_inner() {
        let mock = MockProvider::new("test", test_response());
        let provider = CachingProvider::new(mock, &test_config());

        let resp = provider.chat(&test_request()).await.unwrap();
        assert_eq!(resp.message.content.to_text(), "cached answer");
        assert_eq!(resp.usage.input_tokens, 100);
    }

    #[tokio::test]
    async fn caching_provider_skips_tool_use_responses() {
        let mock = MockProvider::new("test", tool_use_response());
        let provider = CachingProvider::new(mock, &test_config());

        let req = test_request();

        // First call — tool use response, should NOT be cached.
        provider.chat(&req).await.unwrap();

        // Prompt cache should be empty.
        let key = prompt_cache_key(&req);
        assert!(provider.prompt_cache.get(&key).is_none());
    }

    #[tokio::test]
    async fn caching_provider_emits_observer_events() {
        let events: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(vec![]));
        let events_clone = events.clone();
        let observer: CacheObserver = Arc::new(move |event| {
            let label = match &event {
                CacheEvent::PromptCacheHit { .. } => "prompt_hit",
                CacheEvent::SemanticCacheHit { .. } => "semantic_hit",
            };
            events_clone.lock().unwrap().push(label.into());
        });

        let mock = MockProvider::new("test", test_response());
        let provider = CachingProvider::new(mock, &test_config()).with_observer(observer);

        let req = test_request();
        provider.chat(&req).await.unwrap(); // miss
        provider.chat(&req).await.unwrap(); // hit

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], "prompt_hit");
    }

    #[tokio::test]
    async fn caching_provider_metrics_tracking() {
        let mock = MockProvider::new("test", test_response());
        let provider = CachingProvider::new(mock, &test_config());

        let req = test_request();
        provider.chat(&req).await.unwrap(); // miss
        provider.chat(&req).await.unwrap(); // hit

        let m = provider.metrics();
        assert_eq!(m.prompt_hits.load(Ordering::Relaxed), 1);
        assert_eq!(m.prompt_misses.load(Ordering::Relaxed), 1);
        assert_eq!(m.tokens_saved.load(Ordering::Relaxed), 100);
    }

    #[tokio::test]
    async fn caching_provider_delegates_name_model() {
        let mock = MockProvider::new("my-provider", test_response());
        let provider = CachingProvider::new(mock, &test_config());
        assert_eq!(provider.name(), "my-provider");
        assert_eq!(provider.model_name(), "unknown");
    }

    // ---- Helper function tests ----

    #[test]
    fn extract_last_user_message_finds_last() {
        let req = ChatRequest {
            system_prompt: None,
            messages: vec![
                ChatMessage::user("first"),
                ChatMessage::assistant("reply"),
                ChatMessage::user("second"),
            ],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };
        assert_eq!(extract_last_user_message(&req), Some("second"));
    }

    #[test]
    fn extract_last_user_message_empty_returns_none() {
        let req = ChatRequest {
            system_prompt: None,
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };
        assert!(extract_last_user_message(&req).is_none());
    }
}
