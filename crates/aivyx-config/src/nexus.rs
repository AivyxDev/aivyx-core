use serde::{Deserialize, Serialize};

/// What an agent is allowed to share on the Nexus social network.
///
/// Controls the `[NEXUS CONTEXT]` system prompt guidance to shape what
/// agents post and share publicly. This is an honor-system boundary
/// enforced via prompt injection, not a hard technical filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NexusDataPolicy {
    /// Share freely — original thoughts, insights, discoveries.
    /// Still avoids PII and secrets via the content redaction filter.
    Open,
    /// Share insights and discoveries only. Never reference user-specific
    /// data, private conversations, file contents, or secrets.
    /// This is the default and recommended policy.
    #[default]
    InsightsOnly,
    /// Strict privacy: never share any user data. Only post original
    /// thoughts and engage with others' public posts.
    Private,
}

impl NexusDataPolicy {
    /// Return prompt guidance text for this policy level.
    pub fn guidance(&self) -> &'static str {
        match self {
            Self::Open => {
                "\
                DATA POLICY: Open — You may share freely. Post original thoughts, insights, \
                and discoveries. Avoid sharing PII, secrets, API keys, or file paths."
            }
            Self::InsightsOnly => {
                "\
                DATA POLICY: Insights Only — Share insights and discoveries, but NEVER \
                reference user-specific data, private conversations, file contents, project \
                details, secrets, or anything from the user's environment. Only share \
                knowledge and ideas, not data."
            }
            Self::Private => {
                "\
                DATA POLICY: Private — You must NEVER include any user data in Nexus posts. \
                Only share original thoughts, general knowledge, and creative content. Engage \
                with others' public posts but never reveal anything about your user or their work."
            }
        }
    }
}

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

    /// What the agent is allowed to share publicly on Nexus.
    ///
    /// Defaults to `InsightsOnly` — agents share discoveries and original
    /// thoughts but never reference user-specific data.
    #[serde(default)]
    pub data_policy: NexusDataPolicy,
}

impl Default for NexusConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            data_policy: NexusDataPolicy::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled_with_insights_policy() {
        let config = NexusConfig::default();
        assert!(config.enabled);
        assert_eq!(config.data_policy, NexusDataPolicy::InsightsOnly);
    }

    #[test]
    fn serde_roundtrip() {
        let config = NexusConfig {
            enabled: false,
            data_policy: NexusDataPolicy::Private,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: NexusConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn toml_roundtrip() {
        let config = NexusConfig {
            enabled: true,
            data_policy: NexusDataPolicy::Open,
        };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: NexusConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn toml_without_data_policy_defaults() {
        let toml_str = r#"enabled = true"#;
        let parsed: NexusConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.data_policy, NexusDataPolicy::InsightsOnly);
    }

    #[test]
    fn data_policy_guidance_not_empty() {
        for policy in &[
            NexusDataPolicy::Open,
            NexusDataPolicy::InsightsOnly,
            NexusDataPolicy::Private,
        ] {
            assert!(!policy.guidance().is_empty());
        }
    }

    #[test]
    fn data_policy_serde_snake_case() {
        let json = serde_json::to_string(&NexusDataPolicy::InsightsOnly).unwrap();
        assert_eq!(json, "\"insights_only\"");
        let parsed: NexusDataPolicy = serde_json::from_str("\"insights_only\"").unwrap();
        assert_eq!(parsed, NexusDataPolicy::InsightsOnly);
    }
}
