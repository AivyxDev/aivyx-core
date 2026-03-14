use aivyx_core::AutonomyTier;
use serde::{Deserialize, Serialize};

/// Circuit-breaker policy governing agent autonomy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyPolicy {
    /// The autonomy tier assigned to new agents by default.
    pub default_tier: AutonomyTier,
    /// Rate limit for tool invocations.
    pub max_tool_calls_per_minute: u32,
    /// Spending cap per agent session in USD.
    /// 0.0 means unlimited (no cap enforcement) — the natural default for
    /// local providers like Ollama.
    #[serde(default = "default_max_cost")]
    pub max_cost_per_session_usd: f64,
    /// Whether destructive actions always require human approval.
    #[serde(default = "default_require_approval")]
    pub require_approval_for_destructive: bool,
    /// Maximum number of retries for transient LLM errors (rate limit, HTTP).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff.
    #[serde(default = "default_retry_base_delay_ms")]
    pub retry_base_delay_ms: u64,
}

fn default_max_retries() -> u32 {
    3
}

fn default_retry_base_delay_ms() -> u64 {
    1000
}

fn default_max_cost() -> f64 {
    0.0 // unlimited
}

fn default_require_approval() -> bool {
    true // safe default
}

impl Default for AutonomyPolicy {
    fn default() -> Self {
        Self {
            default_tier: AutonomyTier::Leash,
            max_tool_calls_per_minute: 60,
            max_cost_per_session_usd: 5.0,
            require_approval_for_destructive: true,
            max_retries: default_max_retries(),
            retry_base_delay_ms: default_retry_base_delay_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_safe() {
        let policy = AutonomyPolicy::default();
        assert_eq!(policy.default_tier, AutonomyTier::Leash);
        assert!(policy.require_approval_for_destructive);
        assert_eq!(policy.max_tool_calls_per_minute, 60);
    }

    #[test]
    fn toml_roundtrip() {
        let policy = AutonomyPolicy::default();
        let toml_str = toml::to_string(&policy).unwrap();
        let parsed: AutonomyPolicy = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.default_tier, policy.default_tier);
        assert_eq!(
            parsed.max_cost_per_session_usd,
            policy.max_cost_per_session_usd
        );
        assert_eq!(parsed.max_retries, 3);
        assert_eq!(parsed.retry_base_delay_ms, 1000);
    }

    #[test]
    fn backward_compat_missing_retry_fields() {
        let toml_str = r#"
default_tier = "Leash"
max_tool_calls_per_minute = 60
max_cost_per_session_usd = 5.0
require_approval_for_destructive = true
"#;
        let parsed: AutonomyPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.max_retries, 3);
        assert_eq!(parsed.retry_base_delay_ms, 1000);
    }

    #[test]
    fn backward_compat_missing_cost_and_approval() {
        // Minimal config that omits cost cap and approval flag.
        let toml_str = r#"
default_tier = "Trust"
max_tool_calls_per_minute = 120
"#;
        let parsed: AutonomyPolicy = toml::from_str(toml_str).unwrap();
        assert!(
            (parsed.max_cost_per_session_usd - 0.0).abs() < f64::EPSILON,
            "default cost should be 0.0 (unlimited)"
        );
        assert!(
            parsed.require_approval_for_destructive,
            "default should require approval"
        );
        assert_eq!(parsed.max_retries, 3);
        assert_eq!(parsed.retry_base_delay_ms, 1000);
    }
}
