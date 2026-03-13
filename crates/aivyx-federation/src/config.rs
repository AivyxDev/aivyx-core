//! Federation configuration types.

use aivyx_core::AutonomyTier;
use serde::{Deserialize, Serialize};

/// Top-level federation configuration (from `[federation]` in config.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConfig {
    /// Unique identifier for this engine instance (e.g., "vps5-ops").
    pub instance_id: String,

    /// Whether federation is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Path to this instance's Ed25519 private key (PEM or raw).
    /// If absent, a keypair is generated on first boot.
    pub private_key_path: Option<String>,

    /// Configured peers.
    #[serde(default)]
    pub peers: Vec<PeerConfig>,

    /// Failover configuration.
    #[serde(default)]
    pub failover: FailoverConfig,
}

/// Configuration for a single federation peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    /// Unique identifier for the peer (e.g., "vps1-studio").
    pub id: String,

    /// Base URL of the peer engine (e.g., `https://api.aivyx-studio.io`).
    ///
    /// Must use `https://` — plain HTTP is rejected to prevent credential
    /// exposure over unencrypted connections.
    pub url: String,

    /// The peer's Ed25519 public key (base64-encoded).
    /// Used to verify responses from this peer.
    pub public_key: String,

    /// Bearer token to send when authenticating with this peer.
    /// Each peer can require a different token. If absent, no
    /// `Authorization` header is sent (Ed25519 signatures are still used).
    #[serde(default)]
    pub bearer_token: Option<String>,

    /// What this peer exposes to us.
    #[serde(default = "default_capabilities")]
    pub capabilities: Vec<String>,

    /// Trust policy governing what this peer's relayed requests can do on
    /// our instance. `None` means relay requests from this peer are denied.
    #[serde(default)]
    pub trust_policy: Option<TrustPolicy>,
}

/// Trust policy for a federated peer.
///
/// Controls what capabilities a peer's agents receive when their requests
/// are relayed through this instance. Follows the principle of least
/// privilege — only explicitly allowed scopes are granted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustPolicy {
    /// Allowed capability scope names for this peer.
    ///
    /// Examples: `["memory", "filesystem:read"]`.
    /// Only requests matching these scopes will be permitted.
    pub allowed_scopes: Vec<String>,

    /// Maximum autonomy tier for relayed requests.
    ///
    /// Defaults to `Leash` (agent can propose actions but needs confirmation).
    #[serde(default = "default_max_tier")]
    pub max_tier: AutonomyTier,
}

/// Failover configuration for federated relay operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    /// Whether automatic failover is enabled.
    pub enabled: bool,
    /// Maximum number of peers to try before giving up.
    pub max_attempts: usize,
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
        }
    }
}

fn default_max_tier() -> AutonomyTier {
    AutonomyTier::Leash
}

fn default_capabilities() -> Vec<String> {
    vec![
        "chat".to_string(),
        "agents".to_string(),
        "memory".to_string(),
    ]
}

impl FederationConfig {
    /// Returns an empty/disabled config.
    pub fn disabled() -> Self {
        Self {
            instance_id: String::new(),
            enabled: false,
            private_key_path: None,
            peers: Vec::new(),
            failover: FailoverConfig::default(),
        }
    }

    /// Validate federation config. Returns an error if any peer URL uses
    /// plain HTTP (credentials would be exposed) or is malformed.
    pub fn validate(&self) -> Result<(), aivyx_core::AivyxError> {
        for peer in &self.peers {
            if peer.url.starts_with("http://") {
                return Err(aivyx_core::AivyxError::Config(format!(
                    "peer '{}' uses plain HTTP ({}). Federation requires HTTPS \
                     to protect bearer tokens and signatures in transit",
                    peer.id, peer.url,
                )));
            }
            if !peer.url.starts_with("https://") {
                return Err(aivyx_core::AivyxError::Config(format!(
                    "peer '{}' URL must start with https:// (got '{}')",
                    peer.id, peer.url,
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failover_config_defaults() {
        let config = FailoverConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 3);
    }

    #[test]
    fn failover_config_serde_roundtrip() {
        let config = FailoverConfig {
            enabled: false,
            max_attempts: 5,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: FailoverConfig = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.enabled);
        assert_eq!(deserialized.max_attempts, 5);
    }

    #[test]
    fn federation_config_with_failover_default() {
        // When "failover" key is missing, defaults should apply.
        let json = r#"{"instance_id": "test", "private_key_path": null}"#;
        let config: FederationConfig = serde_json::from_str(json).unwrap();
        assert!(config.failover.enabled);
        assert_eq!(config.failover.max_attempts, 3);
    }

    #[test]
    fn trust_policy_max_tier_default() {
        let json = r#"{"allowed_scopes": ["memory"]}"#;
        let policy: TrustPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(policy.max_tier, AutonomyTier::Leash);
    }

    #[test]
    fn trust_policy_max_tier_roundtrip() {
        let policy = TrustPolicy {
            allowed_scopes: vec!["chat".into()],
            max_tier: AutonomyTier::Trust,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: TrustPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_tier, AutonomyTier::Trust);
        assert_eq!(parsed.allowed_scopes, vec!["chat"]);
    }

    #[test]
    fn trust_policy_rejects_invalid_tier() {
        let json = r#"{"allowed_scopes": [], "max_tier": "invalid"}"#;
        assert!(serde_json::from_str::<TrustPolicy>(json).is_err());
    }

    #[test]
    fn validate_rejects_http_peer_url() {
        let config = FederationConfig {
            instance_id: "test".into(),
            enabled: true,
            private_key_path: None,
            peers: vec![PeerConfig {
                id: "insecure".into(),
                url: "http://peer.example.com".into(),
                public_key: "AAAA".into(),
                bearer_token: Some("secret".into()),
                capabilities: vec![],
                trust_policy: None,
            }],
            failover: FailoverConfig::default(),
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("plain HTTP"));
    }

    #[test]
    fn validate_accepts_https_peer_url() {
        let config = FederationConfig {
            instance_id: "test".into(),
            enabled: true,
            private_key_path: None,
            peers: vec![PeerConfig {
                id: "secure".into(),
                url: "https://peer.example.com".into(),
                public_key: "AAAA".into(),
                bearer_token: None,
                capabilities: vec![],
                trust_policy: None,
            }],
            failover: FailoverConfig::default(),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn bearer_token_optional_in_serde() {
        let json = r#"{"id":"p","url":"https://p.com","public_key":"AAAA"}"#;
        let peer: PeerConfig = serde_json::from_str(json).unwrap();
        assert!(peer.bearer_token.is_none());

        let json = r#"{"id":"p","url":"https://p.com","public_key":"AAAA","bearer_token":"tok"}"#;
        let peer: PeerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(peer.bearer_token.as_deref(), Some("tok"));
    }
}
