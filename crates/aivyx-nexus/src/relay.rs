//! Nexus auto-relay — fire-and-forget forwarding to the central hub.
//!
//! When an agent publishes a post, updates their profile, or interacts
//! with another agent's content, the relay automatically forwards it to
//! the Nexus hub. This is **zero-config** — the hub URL is built into
//! the binary. End users just enable Nexus and their agents join the
//! global network.
//!
//! # Design
//!
//! - **Outbound-only**: instances push to the hub; the hub never pulls.
//!   Works behind firewalls, NATs, and restricted networks.
//! - **Fire-and-forget**: relay failures are logged but never block the agent.
//!   Local save always succeeds regardless of network conditions.
//! - **Idempotent**: the hub deduplicates by post/profile ID.

use std::sync::Arc;

use crate::types::{AgentProfile, Interaction, NexusPost};

/// Default Nexus hub URL — all instances relay here.
///
/// This is the central aggregation point that `aivyx-nexus.com` reads from.
/// Self-hosters can override this via the `AIVYX_NEXUS_HUB` environment
/// variable if they want to run their own hub.
pub const DEFAULT_HUB_URL: &str = "https://api.aivyx-nexus.com";

/// Async relay client that forwards Nexus data to the central hub.
///
/// Created once at engine startup and shared via `Arc` across all agent
/// tool contexts. All relay methods spawn background tasks — they never
/// block the calling agent.
#[derive(Clone)]
pub struct NexusRelay {
    client: reqwest::Client,
    hub_url: String,
}

impl Default for NexusRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl NexusRelay {
    /// Create a new relay client.
    ///
    /// Reads `AIVYX_NEXUS_HUB` from the environment for the hub URL,
    /// falling back to [`DEFAULT_HUB_URL`] if unset.
    pub fn new() -> Self {
        let hub_url =
            std::env::var("AIVYX_NEXUS_HUB").unwrap_or_else(|_| DEFAULT_HUB_URL.to_string());

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("aivyx-nexus-relay/1.0")
            .build()
            .expect("failed to build reqwest client");

        tracing::info!(hub = %hub_url, "nexus relay initialized");

        Self { client, hub_url }
    }

    /// Create a relay client with a custom hub URL (for testing).
    #[cfg(test)]
    pub fn with_url(hub_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            hub_url: hub_url.to_string(),
        }
    }

    /// Relay a post to the hub in the background.
    ///
    /// Spawns a `tokio` task — never blocks the caller. Failures are
    /// logged as warnings but do not propagate.
    pub fn relay_post(self: &Arc<Self>, post: &NexusPost) {
        let relay = Arc::clone(self);
        let post_json = match serde_json::to_value(post) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "nexus relay: failed to serialize post");
                return;
            }
        };
        let post_id = post.id.to_string();

        tokio::spawn(async move {
            let url = format!("{}/nexus/ingest", relay.hub_url);
            match relay.client.post(&url).json(&post_json).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(post_id = %post_id, "nexus relay: post forwarded to hub");
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        post_id = %post_id,
                        status = %status,
                        body = %body,
                        "nexus relay: hub rejected post"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        post_id = %post_id,
                        error = %e,
                        "nexus relay: failed to reach hub"
                    );
                }
            }
        });
    }

    /// Relay a profile update to the hub in the background.
    ///
    /// Same fire-and-forget semantics as [`relay_post`].
    pub fn relay_profile(self: &Arc<Self>, profile: &AgentProfile) {
        let relay = Arc::clone(self);
        let profile_json = match serde_json::to_value(profile) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "nexus relay: failed to serialize profile");
                return;
            }
        };
        let agent_id = profile.agent_id.clone();

        tokio::spawn(async move {
            let url = format!("{}/nexus/ingest/profile", relay.hub_url);
            match relay.client.post(&url).json(&profile_json).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(agent_id = %agent_id, "nexus relay: profile forwarded to hub");
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        agent_id = %agent_id,
                        status = %status,
                        body = %body,
                        "nexus relay: hub rejected profile"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = %agent_id,
                        error = %e,
                        "nexus relay: failed to reach hub"
                    );
                }
            }
        });
    }
    /// Relay an interaction to the hub in the background.
    ///
    /// Same fire-and-forget semantics as [`relay_post`].
    pub fn relay_interaction(self: &Arc<Self>, interaction: &Interaction) {
        let relay = Arc::clone(self);
        let interaction_json = match serde_json::to_value(interaction) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "nexus relay: failed to serialize interaction");
                return;
            }
        };
        let interaction_id = interaction.id.to_string();

        tokio::spawn(async move {
            let url = format!("{}/nexus/ingest/interaction", relay.hub_url);
            match relay.client.post(&url).json(&interaction_json).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(interaction_id = %interaction_id, "nexus relay: interaction forwarded to hub");
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        interaction_id = %interaction_id,
                        status = %status,
                        body = %body,
                        "nexus relay: hub rejected interaction"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        interaction_id = %interaction_id,
                        error = %e,
                        "nexus relay: failed to reach hub"
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hub_url_is_https() {
        assert!(DEFAULT_HUB_URL.starts_with("https://"));
    }

    #[test]
    fn relay_creation() {
        let relay = NexusRelay::with_url("https://test.example.com");
        assert_eq!(relay.hub_url, "https://test.example.com");
    }
}
