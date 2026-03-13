//! TTL-based in-memory cache for tool execution results.
//!
//! Caches tool call results to avoid redundant external requests.
//! Particularly useful for web search and HTTP fetch tools where
//! repeated identical queries should return cached results.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Entry in the result cache with expiration tracking.
struct CacheEntry {
    /// Cached JSON result value.
    value: serde_json::Value,
    /// When this entry expires.
    expires_at: Instant,
}

/// Thread-safe, TTL-based result cache for tool executions.
///
/// Cache keys are generated from the tool name + input JSON via SHA-256.
/// Entries expire after a configurable TTL (default: 5 minutes).
/// Capped at `max_entries` to prevent unbounded memory growth.
pub struct ToolResultCache {
    /// Cached entries keyed by hash string.
    entries: Mutex<HashMap<String, CacheEntry>>,
    /// Default time-to-live for cached entries.
    default_ttl: Duration,
    /// Maximum number of entries before eviction is forced.
    max_entries: usize,
}

/// Default maximum cache entries (1024).
const DEFAULT_MAX_ENTRIES: usize = 1024;

impl ToolResultCache {
    /// Create a new cache with the given TTL for entries.
    pub fn new(default_ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            default_ttl,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    /// Override the maximum number of cached entries.
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Look up a cached value by key. Returns `None` if not found or expired.
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get(key) {
            if entry.expires_at > Instant::now() {
                return Some(entry.value.clone());
            }
            // Expired — remove it.
            entries.remove(key);
        }
        None
    }

    /// Insert a value into the cache with the default TTL.
    ///
    /// If the cache is at capacity, expired entries are evicted first.
    /// If still at capacity, the entry closest to expiration is removed.
    pub fn insert(&self, key: &str, value: serde_json::Value) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        // If at capacity and this is a new key, make room.
        if entries.len() >= self.max_entries && !entries.contains_key(key) {
            // First pass: evict expired entries.
            let now = Instant::now();
            entries.retain(|_, entry| entry.expires_at > now);

            // Second pass: if still at capacity, evict the entry nearest expiration.
            if entries.len() >= self.max_entries {
                if let Some(oldest_key) = entries
                    .iter()
                    .min_by_key(|(_, e)| e.expires_at)
                    .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest_key);
                }
            }
        }

        entries.insert(
            key.to_string(),
            CacheEntry {
                value,
                expires_at: Instant::now() + self.default_ttl,
            },
        );
    }

    /// Remove all expired entries from the cache.
    pub fn evict_expired(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        entries.retain(|_, entry| entry.expires_at > now);
    }

    /// Number of entries currently in the cache (including expired).
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Returns true if the cache has no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Generate a deterministic cache key from a tool name and its input JSON.
    ///
    /// Uses SHA-256 of the concatenated tool name and canonical JSON representation.
    pub fn cache_key(tool_name: &str, input: &serde_json::Value) -> String {
        let canonical = serde_json::to_string(input).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(tool_name.as_bytes());
        hasher.update(b":");
        hasher.update(canonical.as_bytes());
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let cache = ToolResultCache::new(Duration::from_secs(60));
        let key = ToolResultCache::cache_key("test", &serde_json::json!({"q": "hello"}));

        assert!(cache.get(&key).is_none());

        cache.insert(&key, serde_json::json!({"result": "world"}));
        let cached = cache.get(&key).unwrap();
        assert_eq!(cached["result"], "world");
    }

    #[test]
    fn expired_entries_return_none() {
        let cache = ToolResultCache::new(Duration::from_millis(1));
        let key = "expired-key";

        cache.insert(key, serde_json::json!("old"));

        // Sleep to let it expire.
        std::thread::sleep(Duration::from_millis(5));

        assert!(cache.get(key).is_none());
    }

    #[test]
    fn evict_expired_removes_old_entries() {
        let cache = ToolResultCache::new(Duration::from_millis(1));
        cache.insert("a", serde_json::json!("val_a"));
        cache.insert("b", serde_json::json!("val_b"));
        assert_eq!(cache.len(), 2);

        std::thread::sleep(Duration::from_millis(5));
        cache.evict_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_key_deterministic() {
        let input = serde_json::json!({"query": "rust async"});
        let k1 = ToolResultCache::cache_key("web_search", &input);
        let k2 = ToolResultCache::cache_key("web_search", &input);
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_tool_name() {
        let input = serde_json::json!({"query": "rust"});
        let k1 = ToolResultCache::cache_key("web_search", &input);
        let k2 = ToolResultCache::cache_key("http_fetch", &input);
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_input() {
        let k1 = ToolResultCache::cache_key("search", &serde_json::json!({"q": "a"}));
        let k2 = ToolResultCache::cache_key("search", &serde_json::json!({"q": "b"}));
        assert_ne!(k1, k2);
    }

    #[test]
    fn len_and_is_empty() {
        let cache = ToolResultCache::new(Duration::from_secs(60));
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        cache.insert("k", serde_json::json!("v"));
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_hit_returns_stored_value() {
        let cache = ToolResultCache::new(Duration::from_secs(300));
        let key = ToolResultCache::cache_key("my_tool", &serde_json::json!({"arg": "val"}));
        let value = serde_json::json!({"output": "computed_result", "count": 42});

        cache.insert(&key, value.clone());
        let retrieved = cache.get(&key).unwrap();

        assert_eq!(retrieved["output"], "computed_result");
        assert_eq!(retrieved["count"], 42);
        assert_eq!(retrieved, value);
    }

    #[test]
    fn cache_evicts_expired_entries() {
        let cache = ToolResultCache::new(Duration::from_millis(1));
        cache.insert("fresh", serde_json::json!("data"));
        assert_eq!(cache.len(), 1);

        std::thread::sleep(Duration::from_millis(10));
        cache.evict_expired();
        assert_eq!(cache.len(), 0);

        // Also verify get returns None for expired
        assert!(cache.get("fresh").is_none());
    }

    #[test]
    fn insert_evicts_when_at_capacity() {
        let cache = ToolResultCache::new(Duration::from_secs(300)).with_max_entries(2);

        cache.insert("a", serde_json::json!("val_a"));
        cache.insert("b", serde_json::json!("val_b"));
        assert_eq!(cache.len(), 2);

        // Third insert should evict the entry nearest expiration ("a", inserted first).
        cache.insert("c", serde_json::json!("val_c"));
        assert_eq!(cache.len(), 2);
        assert!(cache.get("c").is_some());
        assert!(cache.get("b").is_some());
        assert!(cache.get("a").is_none());
    }

    #[test]
    fn insert_prefers_evicting_expired_over_live() {
        let cache = ToolResultCache::new(Duration::from_millis(1)).with_max_entries(2);

        cache.insert("old", serde_json::json!("will_expire"));
        std::thread::sleep(Duration::from_millis(5));

        // Re-create with longer TTL for remaining inserts by using a fresh cache
        // with the same max. Instead, test with mixed expiry by inserting the
        // live entry with a fresh cache that has a long TTL.
        let cache = ToolResultCache::new(Duration::from_secs(300)).with_max_entries(2);
        // Manually insert an already-expired entry.
        {
            let mut entries = cache.entries.lock().unwrap();
            entries.insert(
                "expired".into(),
                super::CacheEntry {
                    value: serde_json::json!("gone"),
                    expires_at: Instant::now() - Duration::from_secs(1),
                },
            );
        }
        cache.insert("live", serde_json::json!("here"));
        assert_eq!(cache.len(), 2);

        // Now insert a third — the expired entry should be evicted, not "live".
        cache.insert("new", serde_json::json!("fresh"));
        assert_eq!(cache.len(), 2);
        assert!(cache.get("live").is_some());
        assert!(cache.get("new").is_some());
    }

    #[test]
    fn update_existing_key_does_not_evict() {
        let cache = ToolResultCache::new(Duration::from_secs(300)).with_max_entries(2);
        cache.insert("a", serde_json::json!(1));
        cache.insert("b", serde_json::json!(2));

        // Updating "a" should not trigger eviction since the key already exists.
        cache.insert("a", serde_json::json!(10));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("a").unwrap(), serde_json::json!(10));
        assert!(cache.get("b").is_some());
    }
}
