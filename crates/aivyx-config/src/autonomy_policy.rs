use aivyx_core::AutonomyTier;
use serde::{Deserialize, Serialize};

/// Circuit-breaker policy governing agent autonomy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyPolicy {
    /// The autonomy tier assigned to new agents by default.
    #[serde(default = "default_tier")]
    pub default_tier: AutonomyTier,
    /// Rate limit for tool invocations.
    #[serde(default = "default_max_tool_calls_per_minute")]
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
    /// Circuit breaker settings for LLM provider resilience.
    ///
    /// Controls when a provider is considered down (failure threshold),
    /// how long to wait before retrying (recovery timeout), and how many
    /// successes are needed to consider it recovered.
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerPolicy,
}

/// Circuit breaker settings for LLM provider resilience.
///
/// Applied per-provider when fallback providers are configured. Controls
/// when a provider's circuit opens (marking it as down) and when it
/// transitions back to healthy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerPolicy {
    /// Consecutive failures before opening the circuit. Default: 3.
    #[serde(default = "default_cb_failure_threshold")]
    pub failure_threshold: u32,
    /// Seconds to wait before probing a failed provider. Default: 30.
    #[serde(default = "default_cb_recovery_timeout_secs")]
    pub recovery_timeout_secs: u64,
    /// Consecutive successes in half-open state needed to close circuit. Default: 1.
    #[serde(default = "default_cb_success_threshold")]
    pub success_threshold: u32,
}

fn default_tier() -> AutonomyTier {
    AutonomyTier::Leash
}

fn default_max_tool_calls_per_minute() -> u32 {
    60
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

fn default_cb_failure_threshold() -> u32 {
    3
}

fn default_cb_recovery_timeout_secs() -> u64 {
    30
}

fn default_cb_success_threshold() -> u32 {
    1
}

impl Default for CircuitBreakerPolicy {
    fn default() -> Self {
        Self {
            failure_threshold: default_cb_failure_threshold(),
            recovery_timeout_secs: default_cb_recovery_timeout_secs(),
            success_threshold: default_cb_success_threshold(),
        }
    }
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
            circuit_breaker: CircuitBreakerPolicy::default(),
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

    #[test]
    fn completely_empty_autonomy_section_deserializes() {
        // All fields have serde defaults, so an empty table must parse.
        let parsed: AutonomyPolicy = toml::from_str("").unwrap();
        assert_eq!(parsed.default_tier, AutonomyTier::Leash);
        assert_eq!(parsed.max_tool_calls_per_minute, 60);
        assert!(parsed.require_approval_for_destructive);
        assert_eq!(parsed.max_retries, 3);
        assert_eq!(parsed.retry_base_delay_ms, 1000);
        assert!((parsed.max_cost_per_session_usd - 0.0).abs() < f64::EPSILON);
        // Circuit breaker defaults present.
        assert_eq!(parsed.circuit_breaker.failure_threshold, 3);
        assert_eq!(parsed.circuit_breaker.recovery_timeout_secs, 30);
        assert_eq!(parsed.circuit_breaker.success_threshold, 1);
    }

    #[test]
    fn circuit_breaker_policy_defaults() {
        let cb = CircuitBreakerPolicy::default();
        assert_eq!(cb.failure_threshold, 3);
        assert_eq!(cb.recovery_timeout_secs, 30);
        assert_eq!(cb.success_threshold, 1);
    }

    #[test]
    fn circuit_breaker_policy_toml_roundtrip() {
        let toml_str = r#"
failure_threshold = 5
recovery_timeout_secs = 60
success_threshold = 2
"#;
        let parsed: CircuitBreakerPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.failure_threshold, 5);
        assert_eq!(parsed.recovery_timeout_secs, 60);
        assert_eq!(parsed.success_threshold, 2);

        let serialized = toml::to_string(&parsed).unwrap();
        let reparsed: CircuitBreakerPolicy = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.failure_threshold, 5);
    }

    #[test]
    fn circuit_breaker_backward_compat() {
        // Existing configs without circuit_breaker section should still parse.
        let toml_str = r#"
default_tier = "Leash"
max_tool_calls_per_minute = 60
max_cost_per_session_usd = 5.0
require_approval_for_destructive = true
max_retries = 3
retry_base_delay_ms = 1000
"#;
        let parsed: AutonomyPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.circuit_breaker.failure_threshold, 3);
        assert_eq!(parsed.circuit_breaker.recovery_timeout_secs, 30);
    }

    #[test]
    fn autonomy_with_circuit_breaker_section() {
        let toml_str = r#"
default_tier = "Trust"
max_tool_calls_per_minute = 120

[circuit_breaker]
failure_threshold = 5
recovery_timeout_secs = 60
success_threshold = 2
"#;
        let parsed: AutonomyPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.default_tier, AutonomyTier::Trust);
        assert_eq!(parsed.circuit_breaker.failure_threshold, 5);
        assert_eq!(parsed.circuit_breaker.recovery_timeout_secs, 60);
        assert_eq!(parsed.circuit_breaker.success_threshold, 2);
    }
}
