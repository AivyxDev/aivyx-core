//! Federation agent tools — `federation_chat` and `federation_search`.
//!
//! These tools let agents autonomously communicate with agents on remote
//! federated engine instances. Requires the `federation` feature.

#[cfg(feature = "federation")]
use std::sync::Arc;

#[cfg(feature = "federation")]
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
#[cfg(feature = "federation")]
use aivyx_federation::client::FederationClient;
#[cfg(feature = "federation")]
use aivyx_federation::types::{FederatedSearchRequest, RelayChatRequest};
#[cfg(feature = "federation")]
use async_trait::async_trait;

/// Built-in tool: send a chat message to an agent on a federated peer instance.
#[cfg(feature = "federation")]
pub struct FederationChatTool {
    id: ToolId,
    client: Arc<FederationClient>,
}

#[cfg(feature = "federation")]
impl FederationChatTool {
    /// Create a new federation chat tool with a shared federation client.
    pub fn new(client: Arc<FederationClient>) -> Self {
        Self {
            id: ToolId::new(),
            client,
        }
    }
}

#[cfg(feature = "federation")]
#[async_trait]
impl Tool for FederationChatTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "federation_chat"
    }

    fn description(&self) -> &str {
        "Send a chat message to an agent on a federated peer engine instance. \
         Use this when you need to communicate with agents running on other servers."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "peer": {
                    "type": "string",
                    "description": "The peer instance ID (e.g. 'vps1-studio')"
                },
                "agent": {
                    "type": "string",
                    "description": "The agent name on the peer to chat with"
                },
                "message": {
                    "type": "string",
                    "description": "The message to send to the remote agent"
                }
            },
            "required": ["peer", "agent", "message"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let peer = input["peer"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("federation_chat: missing 'peer'".into()))?;
        let agent = input["agent"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("federation_chat: missing 'agent'".into()))?;
        let message = input["message"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("federation_chat: missing 'message'".into()))?;

        let req = RelayChatRequest {
            peer_id: peer.to_string(),
            agent: agent.to_string(),
            message: message.to_string(),
            session_id: None,
        };

        let resp = self
            .client
            .relay_chat(&req)
            .await
            .map_err(|e| AivyxError::Agent(format!("federation_chat failed: {e}")))?;

        Ok(serde_json::json!({
            "peer": resp.peer_id,
            "agent": resp.agent,
            "response": resp.response,
            "session_id": resp.session_id,
        }))
    }
}

/// Built-in tool: search memory across federated peer instances.
#[cfg(feature = "federation")]
pub struct FederationSearchTool {
    id: ToolId,
    client: Arc<FederationClient>,
}

#[cfg(feature = "federation")]
impl FederationSearchTool {
    /// Create a new federation search tool with a shared federation client.
    pub fn new(client: Arc<FederationClient>) -> Self {
        Self {
            id: ToolId::new(),
            client,
        }
    }
}

#[cfg(feature = "federation")]
#[async_trait]
impl Tool for FederationSearchTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "federation_search"
    }

    fn description(&self) -> &str {
        "Search across the memory and knowledge of all federated peer engine instances. \
         Useful for finding information that may be stored on other servers."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "peers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional: specific peer IDs to search. Empty searches all peers."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("federation_search: missing 'query'".into()))?;

        let peers: Vec<String> = input["peers"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let limit = input["limit"].as_u64().map(|l| l as usize);

        let req = FederatedSearchRequest {
            query: query.to_string(),
            peers,
            limit,
        };

        let results = self
            .client
            .federated_search(&req)
            .await
            .map_err(|e| AivyxError::Agent(format!("federation_search failed: {e}")))?;

        let result_values: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "peer": r.peer_id,
                    "content": r.content,
                    "score": r.score,
                    "kind": r.kind,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "query": query,
            "result_count": result_values.len(),
            "results": result_values,
        }))
    }
}
