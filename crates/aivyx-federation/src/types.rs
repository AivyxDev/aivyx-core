//! Shared types for federation requests and responses.

use serde::{Deserialize, Serialize};

/// Summary of a peer instance's status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatus {
    pub id: String,
    pub url: String,
    pub healthy: bool,
    pub last_seen: Option<String>,
    pub agents: Vec<String>,
    pub capabilities: Vec<String>,
}

/// Request to relay a chat message to a peer's agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayChatRequest {
    /// The peer instance to relay to.
    pub peer_id: String,
    /// The agent on the peer to chat with.
    pub agent: String,
    /// The message to send.
    pub message: String,
    /// Optional session ID for continuity.
    pub session_id: Option<String>,
}

/// Response from a relayed chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayChatResponse {
    pub peer_id: String,
    pub agent: String,
    pub response: String,
    pub session_id: String,
}

/// Request to create a task on a peer instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayTaskRequest {
    pub peer_id: String,
    pub agent: String,
    pub goal: String,
}

/// Response from a relayed task creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayTaskResponse {
    pub peer_id: String,
    pub task_id: String,
    pub status: String,
}

/// Federated search request — search memory across peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedSearchRequest {
    pub query: String,
    /// Which peers to search. Empty = all peers.
    #[serde(default)]
    pub peers: Vec<String>,
    pub limit: Option<usize>,
}

/// A single result from federated search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedSearchResult {
    pub peer_id: String,
    pub content: String,
    pub score: f64,
    pub kind: String,
}

/// Response from the /federation/ping endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResponse {
    pub instance_id: String,
    pub version: String,
    pub agents: Vec<String>,
}

/// An agent summary as reported by a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAgent {
    pub name: String,
    pub role: String,
}
