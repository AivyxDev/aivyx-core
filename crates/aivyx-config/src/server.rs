//! HTTP server configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the aivyx HTTP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Address to bind to (default: `"127.0.0.1"`).
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// Port to listen on (default: `3000`).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Allowed CORS origins (default: empty — permissive in dev).
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Maximum WebSocket message size in bytes (default: 1 MiB).
    #[serde(default = "default_ws_max_message_size")]
    pub ws_max_message_size: usize,
    /// Per-endpoint rate limiting configuration (default: disabled).
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
}

/// Per-tier rate limit configuration.
///
/// Each tier has a maximum number of requests allowed within a rolling
/// time window. Uses the GCRA algorithm for fair burst handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitTier {
    /// Maximum requests per window (default varies by tier).
    pub max_requests: u32,
    /// Window duration in seconds (default: 60).
    #[serde(default = "default_rate_limit_window")]
    pub window_secs: u64,
}

/// Rate limiting configuration for expensive endpoint tiers.
///
/// When present, enables per-IP rate limiting on three tiers of endpoints:
/// - `llm`: Chat, team runs, digest generation (default: 10 req/60s)
/// - `search`: Memory search, profile extraction (default: 30 req/60s)
/// - `task`: Task creation and resumption (default: 20 req/60s)
///
/// All other protected endpoints remain unmetered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Rate limit for LLM-calling endpoints (`/chat*`, `/teams/*/run*`, `/digest`).
    #[serde(default = "default_llm_tier")]
    pub llm: RateLimitTier,
    /// Rate limit for search endpoints (`/memory/search`, `/memory/profile/extract`).
    #[serde(default = "default_search_tier")]
    pub search: RateLimitTier,
    /// Rate limit for task endpoints (`POST /tasks`, `/tasks/*/resume`).
    #[serde(default = "default_task_tier")]
    pub task: RateLimitTier,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            port: default_port(),
            cors_origins: Vec::new(),
            ws_max_message_size: default_ws_max_message_size(),
            rate_limit: None,
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            llm: default_llm_tier(),
            search: default_search_tier(),
            task: default_task_tier(),
        }
    }
}

/// Default WebSocket message size limit: 1 MiB.
fn default_ws_max_message_size() -> usize {
    1_048_576
}

fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    3000
}

fn default_rate_limit_window() -> u64 {
    60
}

fn default_llm_tier() -> RateLimitTier {
    RateLimitTier {
        max_requests: 10,
        window_secs: 60,
    }
}

fn default_search_tier() -> RateLimitTier {
    RateLimitTier {
        max_requests: 30,
        window_secs: 60,
    }
}

fn default_task_tier() -> RateLimitTier {
    RateLimitTier {
        max_requests: 20,
        window_secs: 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_address, "127.0.0.1");
        assert_eq!(config.port, 3000);
        assert!(config.cors_origins.is_empty());
        assert_eq!(config.ws_max_message_size, 1_048_576);
    }

    #[test]
    fn toml_roundtrip() {
        let config = ServerConfig {
            bind_address: "0.0.0.0".into(),
            port: 8080,
            cors_origins: vec!["http://localhost:5173".into()],
            ..Default::default()
        };
        let s = toml::to_string(&config).unwrap();
        let loaded: ServerConfig = toml::from_str(&s).unwrap();
        assert_eq!(loaded.bind_address, "0.0.0.0");
        assert_eq!(loaded.port, 8080);
        assert_eq!(loaded.cors_origins.len(), 1);
    }

    #[test]
    fn deserialize_without_optional_fields() {
        let s = "";
        let config: ServerConfig = toml::from_str(s).unwrap();
        assert_eq!(config.port, 3000);
        assert_eq!(config.ws_max_message_size, 1_048_576);
    }

    #[test]
    fn ws_max_message_size_roundtrip() {
        let config = ServerConfig {
            ws_max_message_size: 512_000,
            ..Default::default()
        };
        let s = toml::to_string(&config).unwrap();
        let loaded: ServerConfig = toml::from_str(&s).unwrap();
        assert_eq!(loaded.ws_max_message_size, 512_000);
    }

    #[test]
    fn rate_limit_config_defaults() {
        let config = RateLimitConfig::default();
        assert_eq!(config.llm.max_requests, 10);
        assert_eq!(config.llm.window_secs, 60);
        assert_eq!(config.search.max_requests, 30);
        assert_eq!(config.task.max_requests, 20);
    }

    #[test]
    fn rate_limit_config_roundtrip() {
        let config = ServerConfig {
            rate_limit: Some(RateLimitConfig {
                llm: RateLimitTier {
                    max_requests: 5,
                    window_secs: 30,
                },
                search: RateLimitTier {
                    max_requests: 15,
                    window_secs: 120,
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let s = toml::to_string(&config).unwrap();
        let loaded: ServerConfig = toml::from_str(&s).unwrap();
        let rl = loaded.rate_limit.unwrap();
        assert_eq!(rl.llm.max_requests, 5);
        assert_eq!(rl.llm.window_secs, 30);
        assert_eq!(rl.search.max_requests, 15);
        assert_eq!(rl.search.window_secs, 120);
        assert_eq!(rl.task.max_requests, 20); // default preserved
    }

    #[test]
    fn rate_limit_disabled_by_default() {
        let s = "";
        let config: ServerConfig = toml::from_str(s).unwrap();
        assert!(config.rate_limit.is_none());
    }
}
