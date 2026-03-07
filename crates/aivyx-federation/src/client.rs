//! Federation client — manages peer connections, health probing, and discovery.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use aivyx_core::AivyxError;

use crate::auth::FederationAuth;
use crate::config::{FailoverConfig, FederationConfig, PeerConfig};
use crate::types::{PeerAgent, PeerStatus, PingResponse};

/// State for a single peer.
#[derive(Debug)]
struct PeerState {
    config: PeerConfig,
    healthy: bool,
    last_seen: Option<chrono::DateTime<chrono::Utc>>,
    agents: Vec<String>,
}

/// Federation client managing all peer connections.
pub struct FederationClient {
    auth: Arc<FederationAuth>,
    config: FederationConfig,
    http: reqwest::Client,
    peers: RwLock<HashMap<String, PeerState>>,
}

impl FederationClient {
    /// Create a new federation client from config.
    pub fn new(config: FederationConfig, auth: FederationAuth) -> Self {
        let mut peer_map = HashMap::new();
        for peer in &config.peers {
            peer_map.insert(
                peer.id.clone(),
                PeerState {
                    config: peer.clone(),
                    healthy: false,
                    last_seen: None,
                    agents: Vec::new(),
                },
            );
        }

        Self {
            auth: Arc::new(auth),
            config,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("failed to build HTTP client"),
            peers: RwLock::new(peer_map),
        }
    }

    /// Get the instance ID.
    pub fn instance_id(&self) -> &str {
        &self.config.instance_id
    }

    /// Get this instance's public key (base64) for sharing with peers.
    pub fn public_key(&self) -> String {
        self.auth.public_key_base64()
    }

    /// Probe all peers for health and agent availability.
    pub async fn probe_peers(&self) {
        let peer_configs: Vec<PeerConfig> = {
            let peers = self.peers.read().await;
            peers.values().map(|p| p.config.clone()).collect()
        };

        for peer_config in peer_configs {
            match self.ping_peer(&peer_config).await {
                Ok(ping) => {
                    let mut peers = self.peers.write().await;
                    if let Some(state) = peers.get_mut(&peer_config.id) {
                        state.healthy = true;
                        state.last_seen = Some(chrono::Utc::now());
                        state.agents = ping.agents;
                        tracing::debug!(peer = %peer_config.id, "federation peer healthy");
                    }
                }
                Err(e) => {
                    let mut peers = self.peers.write().await;
                    if let Some(state) = peers.get_mut(&peer_config.id) {
                        state.healthy = false;
                        tracing::warn!(peer = %peer_config.id, error = %e, "federation peer unreachable");
                    }
                }
            }
        }
    }

    /// Ping a specific peer.
    async fn ping_peer(&self, peer: &PeerConfig) -> Result<PingResponse, AivyxError> {
        let url = format!("{}/federation/ping", peer.url.trim_end_matches('/'));
        let body = b"";
        let header = self.auth.sign_request(body);

        let resp = self
            .http
            .get(&url)
            .header("X-Federation-Instance", &header.instance_id)
            .header("X-Federation-Timestamp", header.timestamp.to_string())
            .header("X-Federation-Signature", &header.signature)
            .send()
            .await
            .map_err(|e| AivyxError::Other(format!("federation ping failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AivyxError::Other(format!(
                "peer {} returned {}",
                peer.id,
                resp.status()
            )));
        }

        resp.json::<PingResponse>()
            .await
            .map_err(|e| AivyxError::Other(format!("parse ping response: {e}")))
    }

    /// List all peers with their current status.
    pub async fn list_peers(&self) -> Vec<PeerStatus> {
        let peers = self.peers.read().await;
        peers
            .values()
            .map(|p| PeerStatus {
                id: p.config.id.clone(),
                url: p.config.url.clone(),
                healthy: p.healthy,
                last_seen: p.last_seen.map(|t| t.to_rfc3339()),
                agents: p.agents.clone(),
                capabilities: p.config.capabilities.clone(),
            })
            .collect()
    }

    /// Get agents available on a specific peer.
    pub async fn peer_agents(&self, peer_id: &str) -> Result<Vec<PeerAgent>, AivyxError> {
        let peers = self.peers.read().await;
        let peer = peers
            .get(peer_id)
            .ok_or_else(|| AivyxError::Other(format!("unknown peer: {peer_id}")))?;

        if !peer.healthy {
            return Err(AivyxError::Other(format!("peer {peer_id} is not healthy")));
        }

        let url = format!("{}/agents", peer.config.url.trim_end_matches('/'));
        let body = b"";
        let header = self.auth.sign_request(body);

        let resp = self
            .http
            .get(&url)
            .header("X-Federation-Instance", &header.instance_id)
            .header("X-Federation-Timestamp", header.timestamp.to_string())
            .header("X-Federation-Signature", &header.signature)
            .send()
            .await
            .map_err(|e| AivyxError::Other(format!("fetch peer agents: {e}")))?;

        if !resp.status().is_success() {
            return Err(AivyxError::Other(format!(
                "peer {peer_id} agents returned {}",
                resp.status()
            )));
        }

        // Parse the response — the /agents endpoint returns { agents: [...] } or [...]
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AivyxError::Other(format!("parse agents: {e}")))?;

        let agents_array = body
            .get("agents")
            .and_then(|a| a.as_array())
            .or_else(|| body.as_array())
            .cloned()
            .unwrap_or_default();

        let agents = agents_array
            .iter()
            .filter_map(|a| serde_json::from_value::<PeerAgent>(a.clone()).ok())
            .collect();

        Ok(agents)
    }

    /// Get the peer config by ID.
    pub async fn get_peer_config(&self, peer_id: &str) -> Result<PeerConfig, AivyxError> {
        let peers = self.peers.read().await;
        peers
            .get(peer_id)
            .map(|p| p.config.clone())
            .ok_or_else(|| AivyxError::Other(format!("unknown peer: {peer_id}")))
    }

    /// Get a reference to the auth module.
    pub fn auth(&self) -> &FederationAuth {
        &self.auth
    }

    /// Get a reference to the HTTP client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Get the trust policy for a specific peer, if configured.
    pub async fn peer_trust_policy(&self, peer_id: &str) -> Option<crate::config::TrustPolicy> {
        let peers = self.peers.read().await;
        peers
            .get(peer_id)
            .and_then(|p| p.config.trust_policy.clone())
    }

    /// Get the failover configuration.
    pub fn failover_config(&self) -> &FailoverConfig {
        &self.config.failover
    }

    /// Select the best healthy peer that can handle the given capability.
    /// Priority: most recently seen first, then alphabetical as tiebreaker.
    /// Returns `None` if no healthy peers match.
    pub async fn select_peer(&self, capability: &str) -> Option<String> {
        let candidates = self.healthy_peers_for(capability).await;
        candidates.into_iter().next()
    }

    /// List all healthy peers with a given capability, ordered by preference
    /// (most recently seen first).
    pub async fn healthy_peers_for(&self, capability: &str) -> Vec<String> {
        let peers = self.peers.read().await;
        let mut candidates: Vec<_> = peers
            .values()
            .filter(|p| p.healthy && p.config.capabilities.contains(&capability.to_string()))
            .collect();

        // Sort by last_seen descending (most recent first), then alphabetical as tiebreaker.
        candidates.sort_by(|a, b| {
            match (b.last_seen, a.last_seen) {
                (Some(b_time), Some(a_time)) => b_time.cmp(&a_time),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then_with(|| a.config.id.cmp(&b.config.id))
        });

        candidates
            .into_iter()
            .map(|p| p.config.id.clone())
            .collect()
    }

    /// Mark a peer as unhealthy.
    pub async fn mark_unhealthy(&self, peer_id: &str) {
        let mut peers = self.peers.write().await;
        if let Some(state) = peers.get_mut(peer_id) {
            state.healthy = false;
            tracing::warn!(peer = %peer_id, "marked peer unhealthy due to failover");
        }
    }

    /// Set peer health state for testing.
    #[cfg(test)]
    pub(crate) async fn set_peer_healthy(&self, peer_id: &str, healthy: bool, agents: Vec<String>) {
        let mut peers = self.peers.write().await;
        if let Some(state) = peers.get_mut(peer_id) {
            state.healthy = healthy;
            state.agents = agents;
            if healthy {
                state.last_seen = Some(chrono::Utc::now());
            }
        }
    }

    /// Set peer last_seen for testing.
    #[cfg(test)]
    pub(crate) async fn set_peer_last_seen(
        &self,
        peer_id: &str,
        last_seen: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let mut peers = self.peers.write().await;
        if let Some(state) = peers.get_mut(peer_id) {
            state.last_seen = last_seen;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::FederationAuth;
    use crate::config::{FailoverConfig, FederationConfig, PeerConfig};

    fn make_peer(id: &str, capabilities: Vec<&str>) -> PeerConfig {
        PeerConfig {
            id: id.to_string(),
            url: format!("https://{id}.example.com"),
            public_key: "AAAA".to_string(),
            capabilities: capabilities.into_iter().map(String::from).collect(),
            trust_policy: None,
        }
    }

    fn make_client(peers: Vec<PeerConfig>) -> FederationClient {
        let config = FederationConfig {
            instance_id: "test-instance".to_string(),
            enabled: true,
            private_key_path: None,
            peers,
            failover: FailoverConfig::default(),
        };
        let auth = FederationAuth::generate("test-instance".to_string());
        FederationClient::new(config, auth)
    }

    #[tokio::test]
    async fn select_peer_returns_healthy() {
        let client = make_client(vec![
            make_peer("peer-a", vec!["chat"]),
            make_peer("peer-b", vec!["chat"]),
        ]);
        client.set_peer_healthy("peer-a", true, vec![]).await;
        client.set_peer_healthy("peer-b", true, vec![]).await;

        let selected = client.select_peer("chat").await;
        assert!(selected.is_some());
    }

    #[tokio::test]
    async fn select_peer_none_when_all_unhealthy() {
        let client = make_client(vec![
            make_peer("peer-a", vec!["chat"]),
            make_peer("peer-b", vec!["chat"]),
        ]);
        // Peers start unhealthy by default, don't mark any healthy.
        let selected = client.select_peer("chat").await;
        assert!(selected.is_none());
    }

    #[tokio::test]
    async fn select_peer_filters_by_capability() {
        let client = make_client(vec![
            make_peer("peer-a", vec!["chat"]),
            make_peer("peer-b", vec!["memory"]),
        ]);
        client.set_peer_healthy("peer-a", true, vec![]).await;
        client.set_peer_healthy("peer-b", true, vec![]).await;

        let selected = client.select_peer("memory").await;
        assert_eq!(selected, Some("peer-b".to_string()));

        let selected = client.select_peer("chat").await;
        assert_eq!(selected, Some("peer-a".to_string()));

        let selected = client.select_peer("nonexistent").await;
        assert!(selected.is_none());
    }

    #[tokio::test]
    async fn healthy_peers_ordered_by_last_seen() {
        let client = make_client(vec![
            make_peer("peer-a", vec!["chat"]),
            make_peer("peer-b", vec!["chat"]),
            make_peer("peer-c", vec!["chat"]),
        ]);

        // Set all healthy with different last_seen times.
        let now = chrono::Utc::now();
        client.set_peer_healthy("peer-a", true, vec![]).await;
        client
            .set_peer_last_seen("peer-a", Some(now - chrono::Duration::seconds(30)))
            .await;

        client.set_peer_healthy("peer-b", true, vec![]).await;
        client
            .set_peer_last_seen("peer-b", Some(now - chrono::Duration::seconds(10)))
            .await;

        client.set_peer_healthy("peer-c", true, vec![]).await;
        client
            .set_peer_last_seen("peer-c", Some(now - chrono::Duration::seconds(20)))
            .await;

        let peers = client.healthy_peers_for("chat").await;
        assert_eq!(peers, vec!["peer-b", "peer-c", "peer-a"]);
    }

    #[tokio::test]
    async fn mark_unhealthy_updates_state() {
        let client = make_client(vec![make_peer("peer-a", vec!["chat"])]);
        client.set_peer_healthy("peer-a", true, vec![]).await;

        assert!(client.select_peer("chat").await.is_some());

        client.mark_unhealthy("peer-a").await;

        assert!(client.select_peer("chat").await.is_none());
    }
}
