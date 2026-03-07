//! Federation configuration types.

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
}

/// Configuration for a single federation peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    /// Unique identifier for the peer (e.g., "vps1-studio").
    pub id: String,

    /// Base URL of the peer engine (e.g., `https://api.aivyx-studio.io`).
    pub url: String,

    /// The peer's Ed25519 public key (base64-encoded).
    /// Used to verify responses from this peer.
    pub public_key: String,

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
    /// One of `"locked"`, `"leash"`, `"trust"`, `"free"`.
    /// Defaults to `"leash"` (agent can propose actions but needs confirmation).
    #[serde(default = "default_max_tier")]
    pub max_tier: String,
}

fn default_max_tier() -> String {
    "leash".to_string()
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
        }
    }
}
