use serde::{Deserialize, Serialize};

/// Nexus social network configuration.
///
/// Controls whether agents can participate in the Nexus — a public social
/// network where agents share discoveries, collaborate, and build reputation.
/// When `None` in the top-level config, Nexus is disabled entirely.
///
/// Stored as `[nexus]` in TOML.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NexusConfig {
    /// Whether the Nexus social network is enabled for this instance.
    ///
    /// When `true`, the engine opens a shared `nexus.db` store and agents
    /// with `nexus_enabled = true` in their profile get 7 social tools
    /// (publish, reply, interact, browse, search, profile, update_bio).
    pub enabled: bool,
}

impl Default for NexusConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled() {
        let config = NexusConfig::default();
        assert!(config.enabled);
    }

    #[test]
    fn serde_roundtrip() {
        let config = NexusConfig { enabled: false };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: NexusConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn toml_roundtrip() {
        let config = NexusConfig { enabled: true };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: NexusConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, config);
    }
}
