//! Google A2A (Agent-to-Agent) protocol types.
//!
//! Defines the data structures for the A2A protocol specification,
//! enabling Aivyx Engine to participate in the broader agent ecosystem
//! as both an A2A server (serving Agent Cards, accepting tasks) and
//! an A2A client (discovering and delegating to external agents).
//!
//! All types use `camelCase` serde renaming to match the A2A JSON spec.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Agent Card (served at /.well-known/agent.json)
// ---------------------------------------------------------------------------

/// A2A Agent Card — the primary discovery document for an A2A-compatible agent.
///
/// Served at `GET /.well-known/agent.json` (unauthenticated). External agents
/// fetch this to discover what this instance can do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// Human-readable name of the agent service.
    pub name: String,
    /// Description of what this agent does.
    pub description: String,
    /// Base URL for A2A API calls.
    pub url: String,
    /// Version of the agent service.
    pub version: String,
    /// What this agent supports (streaming, push notifications, etc.).
    pub capabilities: AgentCapabilities,
    /// Skills this agent can perform.
    pub skills: Vec<AgentSkill>,
    /// Accepted input content types.
    pub default_input_modes: Vec<String>,
    /// Produced output content types.
    pub default_output_modes: Vec<String>,
    /// Authentication requirements for the A2A API.
    pub authentication: Option<AgentAuthentication>,
}

/// Agent capabilities advertised in the Agent Card.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Whether this agent supports SSE streaming via `tasks/sendSubscribe`.
    pub streaming: bool,
    /// Whether this agent supports webhook push notifications.
    pub push_notifications: bool,
}

/// A skill (capability) that the agent can perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    /// Unique identifier for the skill.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this skill does.
    pub description: String,
}

/// Authentication requirements for the A2A API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAuthentication {
    /// Supported authentication schemes (e.g., `["bearer"]`).
    pub schemes: Vec<String>,
}

// ---------------------------------------------------------------------------
// A2A Task lifecycle
// ---------------------------------------------------------------------------

/// A2A task state — the lifecycle stages of an A2A task.
///
/// Maps to Aivyx's internal `TaskStatus`:
/// - `Planning` / `Planned` → `Submitted`
/// - `Executing` / `Verifying` → `Working`
/// - Approval step pending → `InputRequired`
/// - `Completed` → `Completed`
/// - `Failed` → `Failed`
/// - `Cancelled` → `Canceled` (note: A2A uses single-l spelling)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum A2aTaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
    Canceled,
}

/// A2A task status with optional message and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aTaskStatus {
    /// Current state of the task.
    pub state: A2aTaskState,
    /// Optional message providing context about the current state.
    pub message: Option<A2aMessage>,
    /// ISO 8601 timestamp of this status update.
    pub timestamp: String,
}

/// Full A2A task representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aTask {
    /// Unique task identifier.
    pub id: String,
    /// Current task status.
    pub status: A2aTaskStatus,
    /// Conversation history (if requested).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<A2aMessage>>,
    /// Output artifacts produced by the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<A2aArtifact>>,
    /// Arbitrary metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// A2A Messages and Parts
// ---------------------------------------------------------------------------

/// A2A message — a structured exchange between user and agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aMessage {
    /// Who sent this message.
    pub role: A2aRole,
    /// Content parts of the message.
    pub parts: Vec<A2aPart>,
}

/// Role of a message sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum A2aRole {
    User,
    Agent,
}

/// A single content part within an A2A message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum A2aPart {
    /// Plain text content.
    Text { text: String },
    /// Structured data (JSON).
    Data { data: serde_json::Value },
}

/// An output artifact produced by a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aArtifact {
    /// Optional name for the artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Content parts of the artifact.
    pub parts: Vec<A2aPart>,
}

// ---------------------------------------------------------------------------
// A2A Streaming (tasks/sendSubscribe) types
// ---------------------------------------------------------------------------

/// Server-Sent Event for task status updates.
///
/// Used by `tasks/sendSubscribe` to stream incremental task state changes
/// to the client via SSE.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    /// Task ID this event relates to.
    pub id: String,
    /// Updated task status.
    pub status: A2aTaskStatus,
    /// Whether this is the final event (task reached a terminal state).
    #[serde(rename = "final")]
    pub is_final: bool,
}

// ---------------------------------------------------------------------------
// A2A Push Notifications
// ---------------------------------------------------------------------------

/// Configuration for push notifications on a specific task.
///
/// When set, the A2A server sends a POST request to the specified URL
/// whenever the task's status changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationConfig {
    /// Webhook URL to receive push notifications.
    pub url: String,
    /// Optional authentication token sent as `Authorization: Bearer <token>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 envelope (used by A2A task API)
// ---------------------------------------------------------------------------

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub id: serde_json::Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    /// Create a success response.
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Create an error response.
    pub fn error(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
            id,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_card_serializes_camel_case() {
        let card = AgentCard {
            name: "Aivyx Engine".into(),
            description: "AI agent orchestration".into(),
            url: "https://api.aivyx.io".into(),
            version: "0.4.0".into(),
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: false,
            },
            skills: vec![AgentSkill {
                id: "research".into(),
                name: "Research".into(),
                description: "Web research and analysis".into(),
            }],
            default_input_modes: vec!["text/plain".into()],
            default_output_modes: vec!["text/plain".into()],
            authentication: Some(AgentAuthentication {
                schemes: vec!["bearer".into()],
            }),
        };

        let json = serde_json::to_value(&card).unwrap();
        assert!(json["defaultInputModes"].is_array());
        assert!(json["pushNotifications"].is_null()); // nested in capabilities
        assert_eq!(json["capabilities"]["pushNotifications"], false);
        assert_eq!(json["skills"][0]["id"], "research");
    }

    #[test]
    fn task_state_serializes_kebab_case() {
        let state = A2aTaskState::InputRequired;
        let json = serde_json::to_value(state).unwrap();
        assert_eq!(json, "input-required");
    }

    #[test]
    fn task_state_roundtrip() {
        for state in [
            A2aTaskState::Submitted,
            A2aTaskState::Working,
            A2aTaskState::InputRequired,
            A2aTaskState::Completed,
            A2aTaskState::Failed,
            A2aTaskState::Canceled,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: A2aTaskState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn a2a_message_with_text_part() {
        let msg = A2aMessage {
            role: A2aRole::User,
            parts: vec![A2aPart::Text {
                text: "Hello agent".into(),
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["parts"][0]["type"], "text");
        assert_eq!(json["parts"][0]["text"], "Hello agent");
    }

    #[test]
    fn json_rpc_success_response() {
        let resp = JsonRpcResponse::success(
            serde_json::json!(1),
            serde_json::json!({"task_id": "abc"}),
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["result"]["task_id"], "abc");
        assert!(json["error"].is_null());
    }

    #[test]
    fn json_rpc_error_response() {
        let resp = JsonRpcResponse::error(serde_json::json!(2), -32601, "method not found");
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32601);
        assert_eq!(json["error"]["message"], "method not found");
        assert!(json["result"].is_null());
    }

    #[test]
    fn task_status_update_event_serde() {
        let event = TaskStatusUpdateEvent {
            id: "task-123".into(),
            status: A2aTaskStatus {
                state: A2aTaskState::Working,
                message: Some(A2aMessage {
                    role: A2aRole::Agent,
                    parts: vec![A2aPart::Text {
                        text: "Processing...".into(),
                    }],
                }),
                timestamp: "2026-03-07T12:00:00Z".into(),
            },
            is_final: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["id"], "task-123");
        assert_eq!(json["status"]["state"], "working");
        assert_eq!(json["final"], false);

        let restored: TaskStatusUpdateEvent = serde_json::from_value(json).unwrap();
        assert_eq!(restored.id, "task-123");
        assert!(!restored.is_final);
    }

    #[test]
    fn task_status_update_event_final() {
        let event = TaskStatusUpdateEvent {
            id: "task-456".into(),
            status: A2aTaskStatus {
                state: A2aTaskState::Completed,
                message: None,
                timestamp: "2026-03-07T12:01:00Z".into(),
            },
            is_final: true,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["final"], true);
        assert_eq!(json["status"]["state"], "completed");
    }

    #[test]
    fn push_notification_config_serde() {
        let config = PushNotificationConfig {
            url: "https://example.com/webhook".into(),
            token: Some("secret-token".into()),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["url"], "https://example.com/webhook");
        assert_eq!(json["token"], "secret-token");

        let restored: PushNotificationConfig = serde_json::from_value(json).unwrap();
        assert_eq!(restored.url, "https://example.com/webhook");
        assert_eq!(restored.token.as_deref(), Some("secret-token"));
    }

    #[test]
    fn push_notification_config_without_token() {
        let config = PushNotificationConfig {
            url: "https://example.com/hook".into(),
            token: None,
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["url"], "https://example.com/hook");
        assert!(json.get("token").is_none());
    }
}
