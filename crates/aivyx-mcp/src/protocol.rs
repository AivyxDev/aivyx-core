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
}
