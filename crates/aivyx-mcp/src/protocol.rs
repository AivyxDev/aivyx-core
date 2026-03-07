//! JSON-RPC 2.0 protocol types and MCP-specific message definitions.
//!
//! The Model Context Protocol uses JSON-RPC 2.0 as its wire format.
//! This module defines the request/response types and MCP-specific
//! structures for tool discovery and invocation.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// MCP protocol version supported by this client.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC 2.0 request message.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest {
    /// Always "2.0".
    pub jsonrpc: &'static str,
    /// Request identifier for correlating responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// Method name (e.g., "initialize", "tools/list", "tools/call").
    pub method: String,
    /// Method parameters, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new request with an ID (expects a response).
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id: Some(id),
            method: method.into(),
            params,
        }
    }

    /// Create a notification (no ID, no response expected).
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id: None,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response message.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse {
    /// Request ID this response correlates to.
    pub id: Option<u64>,
    /// Successful result payload.
    pub result: Option<Value>,
    /// Error payload (mutually exclusive with result).
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Returns the result value or an error if the response indicates failure.
    pub fn into_result(self) -> aivyx_core::Result<Value> {
        if let Some(error) = self.error {
            return Err(aivyx_core::AivyxError::Other(format!(
                "JSON-RPC error {}: {}",
                error.code, error.message
            )));
        }
        self.result.ok_or_else(|| {
            aivyx_core::AivyxError::Other("JSON-RPC response has neither result nor error".into())
        })
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    /// Numeric error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Additional error data.
    pub data: Option<Value>,
}

/// MCP tool definition as returned by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    /// Tool name (used for `tools/call`).
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// JSON Schema for the tool's input parameters.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// MCP `initialize` response result.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Protocol version the server supports.
    pub protocol_version: String,
    /// Server capability declarations.
    pub capabilities: Value,
    /// Server identity information.
    pub server_info: McpServerInfo,
}

/// MCP server identity.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerInfo {
    /// Server name.
    pub name: String,
    /// Server version string.
    pub version: Option<String>,
}

/// MCP `tools/call` result content item.
#[derive(Debug, Clone, Deserialize)]
pub struct McpContent {
    /// Content type (usually "text").
    #[serde(rename = "type")]
    pub content_type: String,
    /// Text content.
    pub text: Option<String>,
}

/// MCP `tools/call` response result.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    /// Result content items.
    pub content: Vec<McpContent>,
    /// Whether the tool call resulted in an error.
    #[serde(default)]
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// MCP Sampling protocol types (server → client)
// ---------------------------------------------------------------------------

/// Incoming JSON-RPC request from the MCP server (e.g., `sampling/createMessage`).
///
/// Distinguished from `JsonRpcResponse` by having a `method` field instead of
/// `result`/`error`. The stdio transport reader must differentiate between these.
#[derive(Debug, Clone, Deserialize)]
pub struct IncomingJsonRpcRequest {
    /// Request ID from the server (must be echoed in the response).
    pub id: Option<u64>,
    /// Method name (e.g., "sampling/createMessage").
    pub method: String,
    /// Request parameters.
    pub params: Option<Value>,
}

/// A generic JSON-RPC message that could be either a response or an incoming request.
///
/// Used by the stdio transport reader to distinguish between server responses
/// (to our requests) and server-initiated requests (e.g., sampling).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// A response to one of our requests (has `result` or `error`).
    Response(JsonRpcResponse),
    /// An incoming request from the server (has `method`).
    Request(IncomingJsonRpcRequest),
}

/// MCP `sampling/createMessage` request parameters.
///
/// Sent by the MCP server when it needs an LLM completion from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingRequest {
    /// Messages to send to the LLM.
    pub messages: Vec<SamplingMessage>,
    /// Optional model preferences.
    #[serde(default)]
    pub model_preferences: Option<Value>,
    /// Optional system prompt.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// A message in a sampling request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingMessage {
    /// Role: "user" or "assistant".
    pub role: String,
    /// Message content.
    pub content: SamplingContent,
}

/// Content of a sampling message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SamplingContent {
    /// Text content.
    Text { text: String },
}

/// MCP `sampling/createMessage` response — returned by the client to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingResponse {
    /// Role of the generated message (always "assistant").
    pub role: String,
    /// Generated content.
    pub content: SamplingContent,
    /// Model that generated the response.
    pub model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialization() {
        let req = JsonRpcRequest::new(1, "tools/list", Some(serde_json::json!({})));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"tools/list\""));
    }

    #[test]
    fn notification_has_no_id() {
        let notif = JsonRpcRequest::notification("notifications/initialized", None);
        let json = serde_json::to_string(&notif).unwrap();
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn response_into_result_success() {
        let resp = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({"tools": []})),
            error: None,
        };
        assert!(resp.into_result().is_ok());
    }

    #[test]
    fn response_into_result_error() {
        let resp = JsonRpcResponse {
            id: Some(1),
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid Request".into(),
                data: None,
            }),
        };
        let err = resp.into_result().unwrap_err();
        assert!(err.to_string().contains("Invalid Request"));
    }

    #[test]
    fn tool_def_deserialization() {
        let json = r#"{
            "name": "echo",
            "description": "Echoes input",
            "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}}
        }"#;
        let def: McpToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.name, "echo");
        assert_eq!(def.description.as_deref(), Some("Echoes input"));
    }

    #[test]
    fn call_tool_result_deserialization() {
        let json = r#"{"content": [{"type": "text", "text": "hello"}], "isError": false}"#;
        let result: CallToolResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text.as_deref(), Some("hello"));
        assert!(!result.is_error);
    }

    #[test]
    fn json_rpc_message_parses_response() {
        let json = r#"{"id": 1, "result": {"tools": []}, "error": null}"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, JsonRpcMessage::Response(_)));
    }

    #[test]
    fn json_rpc_message_parses_incoming_request() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 42,
            "method": "sampling/createMessage",
            "params": {
                "messages": [{"role": "user", "content": {"type": "text", "text": "hello"}}],
                "maxTokens": 100
            }
        }"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        match msg {
            JsonRpcMessage::Request(req) => {
                assert_eq!(req.method, "sampling/createMessage");
                assert_eq!(req.id, Some(42));
            }
            _ => panic!("expected Request variant"),
        }
    }

    #[test]
    fn sampling_request_deserialization() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": {"type": "text", "text": "What is 2+2?"}}
            ],
            "systemPrompt": "You are a calculator",
            "maxTokens": 50
        }"#;
        let req: SamplingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.system_prompt.as_deref(), Some("You are a calculator"));
        assert_eq!(req.max_tokens, Some(50));
    }

    #[test]
    fn sampling_response_serialization() {
        let resp = SamplingResponse {
            role: "assistant".into(),
            content: SamplingContent::Text {
                text: "4".into(),
            },
            model: "claude-sonnet-4-20250514".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"]["type"], "text");
        assert_eq!(json["content"]["text"], "4");
        assert_eq!(json["model"], "claude-sonnet-4-20250514");
    }
}
