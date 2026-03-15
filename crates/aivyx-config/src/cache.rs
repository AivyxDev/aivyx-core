use serde::{Deserialize, Serialize};

/// LLM response caching configuration.
///
/// Controls prompt-level (exact match) and semantic (embedding similarity)
/// caching to reduce redundant LLM calls. Stored as `[cache]` in TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Whether caching is enabled at all.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// TTL in seconds for cached responses. Default: 3600 (1 hour).
    #[serde(default = "default_ttl_secs")]
    pub ttl_secs: u64,
    /// Maximum number of cached prompt entries. Default: 512.
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
    /// Whether semantic (embedding-based) caching is enabled.
    /// Requires an embedding provider to be configured.
    #[serde(default)]
    pub semantic_enabled: bool,
    /// Cosine similarity threshold for semantic cache hits.
    /// Must be between 0.0 and 1.0. Default: 0.95.
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,
    /// Maximum number of semantic cache entries. Default: 256.
    #[serde(default = "default_semantic_max_entries")]
    pub semantic_max_entries: usize,
}

fn default_enabled() -> bool {
    true
}

fn default_ttl_secs() -> u64 {
    3600
}

fn default_max_entries() -> usize {
    512
}

fn default_similarity_threshold() -> f32 {
    0.95
}

fn default_semantic_max_entries() -> usize {
    256
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            ttl_secs: default_ttl_secs(),
            max_entries: default_max_entries(),
            semantic_enabled: false,
            similarity_threshold: default_similarity_threshold(),
            semantic_max_entries: default_semantic_max_entries(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = CacheConfig::default();
        assert!(config.enabled);
        assert_eq!(config.ttl_secs, 3600);
        assert_eq!(config.max_entries, 512);
        assert!(!config.semantic_enabled);
        assert!((config.similarity_threshold - 0.95).abs() < f32::EPSILON);
        assert_eq!(config.semantic_max_entries, 256);
    }

    #[test]
    fn serde_roundtrip() {
        let config = CacheConfig {
            enabled: true,
            ttl_secs: 1800,
            max_entries: 1024,
            semantic_enabled: true,
            similarity_threshold: 0.90,
            semantic_max_entries: 128,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: CacheConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ttl_secs, 1800);
        assert_eq!(parsed.max_entries, 1024);
        assert!(parsed.semantic_enabled);
        assert!((parsed.similarity_threshold - 0.90).abs() < f32::EPSILON);
    }

    #[test]
    fn toml_roundtrip() {
        let config = CacheConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: CacheConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.ttl_secs, 3600);
        assert_eq!(parsed.max_entries, 512);
    }

    #[test]
    fn empty_section_uses_defaults() {
        let parsed: CacheConfig = toml::from_str("").unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.ttl_secs, 3600);
        assert_eq!(parsed.max_entries, 512);
        assert!(!parsed.semantic_enabled);
        assert!((parsed.similarity_threshold - 0.95).abs() < f32::EPSILON);
        assert_eq!(parsed.semantic_max_entries, 256);
    }
}
