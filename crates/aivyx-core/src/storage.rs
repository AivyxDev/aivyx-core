//! Storage backend traits and stub implementations.
//!
//! [`StorageBackend`] abstracts key-value storage so that the same higher-level
//! stores (`MemoryStore`, `SessionStore`, etc.) can be backed by encrypted
//! SQLite/redb today and PostgreSQL or Redis tomorrow.
//!
//! [`SessionCacheBackend`] abstracts session caching for future Redis support.

use crate::Result;
use serde::{Serialize, Deserialize};

/// Trait abstracting key-value storage operations.
///
/// Implementations include the default encrypted redb backend and
/// future PostgreSQL/Redis backends. All values are opaque byte slices —
/// encryption is handled by the caller (MemoryStore, SessionStore, etc.).
pub trait StorageBackend: Send + Sync {
    /// Store a value under the given key.
    fn put(&self, key: &str, value: &[u8]) -> Result<()>;

    /// Retrieve a value by key. Returns `None` if not found.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Delete a key-value pair.
    fn delete(&self, key: &str) -> Result<()>;

    /// List all keys in the store.
    fn list_keys(&self) -> Result<Vec<String>>;
}

/// PostgreSQL storage backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresConfig {
    /// Connection URL (e.g., `postgres://user:pass@host:5432/aivyx`).
    pub connection_url: String,
    /// Maximum number of connections in the pool.
    pub max_connections: u32,
    /// Minimum number of idle connections to maintain.
    pub min_connections: u32,
    /// Connection timeout in seconds.
    pub connect_timeout_secs: u64,
    /// Database schema name.
    pub schema: String,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            connection_url: String::new(),
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 5,
            schema: "aivyx".into(),
        }
    }
}

/// PostgreSQL storage backend.
///
/// Stores key-value pairs in a `kv_store` table:
/// ```sql
/// CREATE TABLE IF NOT EXISTS {schema}.kv_store (
///     key TEXT PRIMARY KEY,
///     value BYTEA NOT NULL,
///     created_at TIMESTAMPTZ DEFAULT NOW(),
///     updated_at TIMESTAMPTZ DEFAULT NOW()
/// );
/// ```
///
/// Requires the `postgres` feature flag (adds `sqlx` dependency).
pub struct PostgresBackend {
    config: PostgresConfig,
}

impl PostgresBackend {
    /// Create a new PostgreSQL backend with the given configuration.
    pub fn new(config: PostgresConfig) -> Self {
        Self { config }
    }

    /// Get the configuration.
    pub fn config(&self) -> &PostgresConfig {
        &self.config
    }
}

impl StorageBackend for PostgresBackend {
    fn put(&self, _key: &str, _value: &[u8]) -> Result<()> {
        Err(crate::AivyxError::Other(
            "PostgresBackend not yet implemented".into(),
        ))
    }

    fn get(&self, _key: &str) -> Result<Option<Vec<u8>>> {
        Err(crate::AivyxError::Other(
            "PostgresBackend not yet implemented".into(),
        ))
    }

    fn delete(&self, _key: &str) -> Result<()> {
        Err(crate::AivyxError::Other(
            "PostgresBackend not yet implemented".into(),
        ))
    }

    fn list_keys(&self) -> Result<Vec<String>> {
        Err(crate::AivyxError::Other(
            "PostgresBackend not yet implemented".into(),
        ))
    }
}

/// Trait for session caching backends.
///
/// Implementations include the default in-memory/file-based session store
/// and a future Redis-backed cache.
#[async_trait::async_trait]
pub trait SessionCacheBackend: Send + Sync {
    /// Retrieve a cached session by key.
    async fn get_session(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Cache a session.
    async fn put_session(&self, key: &str, value: &[u8]) -> Result<()>;

    /// Invalidate a cached session.
    async fn invalidate(&self, key: &str) -> Result<()>;
}

/// Redis session cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    /// Redis connection URL (e.g., `redis://host:6379`).
    pub url: String,
    /// Optional password for authentication.
    pub password: Option<String>,
    /// Redis database number.
    pub db: u8,
    /// Connection pool size.
    pub pool_size: u32,
    /// Default TTL for cached sessions in seconds.
    pub default_ttl_secs: u64,
    /// Key prefix for session keys.
    pub key_prefix: String,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".into(),
            password: None,
            db: 0,
            pool_size: 5,
            default_ttl_secs: 3600,
            key_prefix: "aivyx:session:".into(),
        }
    }
}

/// Redis session cache backend.
///
/// Caches sessions in Redis with configurable TTL. Keys are prefixed with
/// `{key_prefix}` to avoid collisions with other Redis users.
///
/// Requires the `redis` feature flag (adds `fred` or `redis-rs` dependency).
pub struct RedisSessionCache {
    config: RedisConfig,
}

impl RedisSessionCache {
    /// Create a new Redis session cache with the given configuration.
    pub fn new(config: RedisConfig) -> Self {
        Self { config }
    }

    /// Get the configuration.
    pub fn config(&self) -> &RedisConfig {
        &self.config
    }
}

#[async_trait::async_trait]
impl SessionCacheBackend for RedisSessionCache {
    async fn get_session(&self, _key: &str) -> Result<Option<Vec<u8>>> {
        Err(crate::AivyxError::Other(
            "RedisSessionCache not yet implemented".into(),
        ))
    }

    async fn put_session(&self, _key: &str, _value: &[u8]) -> Result<()> {
        Err(crate::AivyxError::Other(
            "RedisSessionCache not yet implemented".into(),
        ))
    }

    async fn invalidate(&self, _key: &str) -> Result<()> {
        Err(crate::AivyxError::Other(
            "RedisSessionCache not yet implemented".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_backend_is_object_safe() {
        // Compile-time check: StorageBackend can be used as a trait object.
        let _: Option<Box<dyn StorageBackend>> = None;
    }

    #[test]
    fn session_cache_backend_is_object_safe() {
        // Compile-time check: SessionCacheBackend can be used as a trait object.
        let _: Option<Box<dyn SessionCacheBackend>> = None;
    }

    #[test]
    fn postgres_backend_put_returns_error() {
        let backend = PostgresBackend::new(PostgresConfig::default());
        let err = backend.put("key", b"value").unwrap_err();
        assert!(
            err.to_string().contains("PostgresBackend not yet implemented"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn postgres_backend_get_returns_error() {
        let backend = PostgresBackend::new(PostgresConfig::default());
        let err = backend.get("key").unwrap_err();
        assert!(
            err.to_string().contains("PostgresBackend not yet implemented"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn postgres_backend_delete_returns_error() {
        let backend = PostgresBackend::new(PostgresConfig::default());
        let err = backend.delete("key").unwrap_err();
        assert!(
            err.to_string().contains("PostgresBackend not yet implemented"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn postgres_backend_list_keys_returns_error() {
        let backend = PostgresBackend::new(PostgresConfig::default());
        let err = backend.list_keys().unwrap_err();
        assert!(
            err.to_string().contains("PostgresBackend not yet implemented"),
            "unexpected error: {err}",
        );
    }

    #[tokio::test]
    async fn redis_session_cache_get_returns_error() {
        let cache = RedisSessionCache::new(RedisConfig::default());
        let err = cache.get_session("key").await.unwrap_err();
        assert!(
            err.to_string()
                .contains("RedisSessionCache not yet implemented"),
            "unexpected error: {err}",
        );
    }

    #[tokio::test]
    async fn redis_session_cache_put_returns_error() {
        let cache = RedisSessionCache::new(RedisConfig::default());
        let err = cache.put_session("key", b"value").await.unwrap_err();
        assert!(
            err.to_string()
                .contains("RedisSessionCache not yet implemented"),
            "unexpected error: {err}",
        );
    }

    #[tokio::test]
    async fn redis_session_cache_invalidate_returns_error() {
        let cache = RedisSessionCache::new(RedisConfig::default());
        let err = cache.invalidate("key").await.unwrap_err();
        assert!(
            err.to_string()
                .contains("RedisSessionCache not yet implemented"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn postgres_config_defaults() {
        let config = PostgresConfig::default();
        assert!(config.connection_url.is_empty());
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 1);
        assert_eq!(config.connect_timeout_secs, 5);
        assert_eq!(config.schema, "aivyx");
    }

    #[test]
    fn postgres_config_serde_roundtrip() {
        let config = PostgresConfig {
            connection_url: "postgres://localhost:5432/aivyx".into(),
            max_connections: 20,
            min_connections: 2,
            connect_timeout_secs: 10,
            schema: "custom".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: PostgresConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.connection_url, "postgres://localhost:5432/aivyx");
        assert_eq!(parsed.max_connections, 20);
        assert_eq!(parsed.schema, "custom");
    }

    #[test]
    fn postgres_backend_stores_config() {
        let config = PostgresConfig {
            connection_url: "postgres://test".into(),
            ..PostgresConfig::default()
        };
        let backend = PostgresBackend::new(config);
        assert_eq!(backend.config().connection_url, "postgres://test");
        assert_eq!(backend.config().schema, "aivyx");
    }

    #[test]
    fn redis_config_defaults() {
        let config = RedisConfig::default();
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert!(config.password.is_none());
        assert_eq!(config.db, 0);
        assert_eq!(config.pool_size, 5);
        assert_eq!(config.default_ttl_secs, 3600);
        assert_eq!(config.key_prefix, "aivyx:session:");
    }

    #[test]
    fn redis_config_serde_roundtrip() {
        let config = RedisConfig {
            url: "redis://prod:6380".into(),
            password: Some("secret".into()),
            db: 2,
            pool_size: 10,
            default_ttl_secs: 7200,
            key_prefix: "myapp:".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RedisConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.url, "redis://prod:6380");
        assert_eq!(parsed.password.as_deref(), Some("secret"));
        assert_eq!(parsed.db, 2);
        assert_eq!(parsed.key_prefix, "myapp:");
    }

    #[test]
    fn redis_session_cache_stores_config() {
        let config = RedisConfig {
            url: "redis://test:6379".into(),
            ..RedisConfig::default()
        };
        let cache = RedisSessionCache::new(config);
        assert_eq!(cache.config().url, "redis://test:6379");
        assert_eq!(cache.config().default_ttl_secs, 3600);
    }
}
