//! Memory subsystem configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the memory subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Maximum number of memories to keep.
    ///
    /// When the limit is exceeded, the oldest and least-accessed memories are
    /// pruned. A value of `0` means unlimited (no pruning).
    #[serde(default = "default_max_memories")]
    pub max_memories: usize,

    /// Number of new facts to accumulate before triggering automatic profile
    /// extraction.
    ///
    /// Set to `0` to disable automatic extraction (manual only via
    /// `aivyx memory profile extract`).
    #[serde(default = "default_profile_extraction_threshold")]
    pub profile_extraction_threshold: u64,

    /// Maximum session age in hours before expiry (default: 720 = 30 days).
    ///
    /// Sessions older than this threshold are automatically deleted when an
    /// attempt is made to load them. Set to `0` for no expiry.
    #[serde(default = "default_session_max_age_hours")]
    pub session_max_age_hours: u64,
}

fn default_max_memories() -> usize {
    1000
}

fn default_profile_extraction_threshold() -> u64 {
    20
}

fn default_session_max_age_hours() -> u64 {
    720
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_memories: default_max_memories(),
            profile_extraction_threshold: default_profile_extraction_threshold(),
            session_max_age_hours: default_session_max_age_hours(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_limit_1000() {
        let config = MemoryConfig::default();
        assert_eq!(config.max_memories, 1000);
    }

    #[test]
    fn default_threshold_is_20() {
        let config = MemoryConfig::default();
        assert_eq!(config.profile_extraction_threshold, 20);
    }

    #[test]
    fn default_session_max_age_is_720() {
        let config = MemoryConfig::default();
        assert_eq!(config.session_max_age_hours, 720);
    }

    #[test]
    fn toml_roundtrip() {
        let config = MemoryConfig {
            max_memories: 500,
            profile_extraction_threshold: 30,
            session_max_age_hours: 48,
        };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: MemoryConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.max_memories, 500);
        assert_eq!(parsed.profile_extraction_threshold, 30);
        assert_eq!(parsed.session_max_age_hours, 48);
    }

    #[test]
    fn toml_missing_field_uses_default() {
        let parsed: MemoryConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.max_memories, 1000);
        assert_eq!(parsed.profile_extraction_threshold, 20);
        assert_eq!(parsed.session_max_age_hours, 720);
    }
}
