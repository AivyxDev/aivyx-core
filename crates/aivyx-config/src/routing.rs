//! Smart model routing configuration.
//!
//! Maps request complexity levels to named LLM providers from the
//! `[providers]` table. When configured, the `RoutingProvider` wrapper
//! classifies each `ChatRequest` and routes it to the cheapest adequate
//! model, achieving significant cost savings on mixed workloads.

use serde::{Deserialize, Serialize};

/// Configuration for complexity-based model routing.
///
/// Each field names a provider from the top-level `[providers]` map.
/// `None` means that complexity level falls back to the agent's default
/// provider.
///
/// # Example
///
/// ```toml
/// [routing]
/// simple = "haiku"
/// medium = "sonnet"
/// complex = "opus"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RoutingConfig {
    /// Named provider for simple requests (short Q&A, lookups).
    #[serde(default)]
    pub simple: Option<String>,
    /// Named provider for medium requests (standard tool use, moderate context).
    #[serde(default)]
    pub medium: Option<String>,
    /// Named provider for complex requests (multi-step reasoning, large context).
    #[serde(default)]
    pub complex: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_none() {
        let config = RoutingConfig::default();
        assert!(config.simple.is_none());
        assert!(config.medium.is_none());
        assert!(config.complex.is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let config = RoutingConfig {
            simple: Some("haiku".into()),
            medium: Some("sonnet".into()),
            complex: Some("opus".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RoutingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn absent_section_backward_compat() {
        // Deserializing from empty JSON object should give defaults
        let parsed: RoutingConfig = serde_json::from_str("{}").unwrap();
        assert!(parsed.simple.is_none());
        assert!(parsed.medium.is_none());
        assert!(parsed.complex.is_none());
    }
}
