//! MCP client for connecting to servers, discovering tools, and invoking them.
//!
//! The [`McpClient`] handles the full MCP lifecycle: connection, initialization,
//! tool discovery via `tools/list`, and tool invocation via `tools/call`.

use std::sync::atomic::{AtomicU64, Ordering};

use aivyx_config::{McpServerConfig, McpTransport as McpTransportConfig};
use aivyx_core::{AivyxError, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::auth::OAuthTokenManager;
use crate::protocol::{
    CallToolResult, ElicitationRequest, ElicitationResponse, InitializeResult, JsonRpcRequest,
    MCP_PROTOCOL_VERSION, McpTaskStatus, McpToolDef, SamplingRequest, SamplingResponse,
};
use crate::streamable::StreamableHttpTransport;
use crate::transport::{McpTransportLayer, SseTransport, StdioTransport};

/// Handler for MCP server-initiated sampling requests.
///
/// When an MCP server sends a `sampling/createMessage` request, the client
/// dispatches it to this handler to generate an LLM completion. The handler
/// is provided by the caller (typically the agent session, which has access
/// to the LLM provider).
#[async_trait]
pub trait SamplingHandler: Send + Sync {
    /// Generate a completion for the given sampling request.
    ///
    /// The implementation should call the LLM provider and return the
    /// assistant's response.
    async fn create_message(&self, request: SamplingRequest) -> Result<SamplingResponse>;
}

/// Handler for MCP server-initiated elicitation requests.
///
/// When an MCP server sends an `elicitation/create` request, the client
/// dispatches it to this handler to collect structured user input. In headless
/// or background modes, the default implementation should auto-dismiss.
#[async_trait]
pub trait ElicitationHandler: Send + Sync {
    /// Handle an elicitation request by presenting it to the user (or auto-dismissing).
    async fn elicit(&self, request: ElicitationRequest) -> Result<ElicitationResponse>;
}

/// Default elicitation handler that auto-dismisses all requests.
///
/// Used in headless/background modes where no interactive user is available.
pub struct AutoDismissElicitationHandler;

#[async_trait]
impl ElicitationHandler for AutoDismissElicitationHandler {
    async fn elicit(&self, _request: ElicitationRequest) -> Result<ElicitationResponse> {
        Ok(ElicitationResponse {
            action: crate::protocol::ElicitationAction::Dismiss,
            content: None,
        })
    }
}

/// MCP client that manages communication with a single MCP server.
///
/// Handles the JSON-RPC lifecycle: connect → initialize → discover tools → call tools.
pub struct McpClient {
    /// Underlying transport (stdio or SSE).
    transport: Box<dyn McpTransportLayer>,
    /// Server name from configuration (used for logging and capability scoping).
    server_name: String,
    /// Monotonically increasing request ID counter.
    next_id: AtomicU64,
}

impl McpClient {
    /// Connect to an MCP server using the given configuration.
    ///
    /// Creates the appropriate transport (stdio or SSE) based on the config
    /// but does NOT call `initialize` — call [`McpClient::initialize`] separately.
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        let timeout = std::time::Duration::from_secs(config.timeout_secs);
        let transport: Box<dyn McpTransportLayer> = match &config.transport {
            McpTransportConfig::Stdio { command, args } => {
                let t = StdioTransport::spawn(command, args, &config.env, timeout).await?;
                Box::new(t)
            }
            McpTransportConfig::Sse { url, auth } => {
                let t = if let Some(auth_config) = auth {
                    match &auth_config.method {
                        aivyx_config::McpAuthMethod::Bearer { token_secret_name } => {
                            // Bearer auth: token_secret_name is treated as the raw token
                            // for now. In production, this would be resolved from the
                            // encrypted store by the caller before connecting.
                            tracing::debug!(
                                server = %config.name,
                                "MCP SSE transport with bearer auth (secret: {token_secret_name})"
                            );
                            SseTransport::new(url, timeout)
                        }
                        aivyx_config::McpAuthMethod::OAuth {
                            client_id, scopes, ..
                        } => {
                            tracing::debug!(
                                server = %config.name,
                                client_id = %client_id,
                                scopes = ?scopes,
                                "MCP SSE transport with OAuth (token must be provided at runtime)"
                            );
                            SseTransport::new(url, timeout)
                        }
                    }
                } else {
                    SseTransport::new(url, timeout)
                };
                Box::new(t)
            }
            McpTransportConfig::StreamableHttp { url, auth } => {
                // Streamable HTTP uses the same SseTransport as a fallback
                // until StreamableHttpTransport is wired in via connect_with_handlers().
                let t = if let Some(auth_config) = auth {
                    match &auth_config.method {
                        aivyx_config::McpAuthMethod::Bearer { token_secret_name } => {
                            tracing::debug!(
                                server = %config.name,
                                "MCP Streamable HTTP with bearer auth (secret: {token_secret_name})"
                            );
                            SseTransport::new(url, timeout)
                        }
                        aivyx_config::McpAuthMethod::OAuth { .. } => {
                            tracing::debug!(
                                server = %config.name,
                                "MCP Streamable HTTP with OAuth — use connect_with_handlers() for full support"
                            );
                            SseTransport::new(url, timeout)
                        }
                    }
                } else {
                    SseTransport::new(url, timeout)
                };
                Box::new(t)
            }
        };

        Ok(Self {
            transport,
            server_name: config.name.clone(),
            next_id: AtomicU64::new(1),
        })
    }

    /// Create an McpClient from an existing transport (useful for testing).
    pub fn from_transport(
        transport: Box<dyn McpTransportLayer>,
        server_name: impl Into<String>,
    ) -> Self {
        Self {
            transport,
            server_name: server_name.into(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Connect to an MCP server with full handler support.
    ///
    /// Unlike [`connect`](McpClient::connect), this method wires sampling and
    /// elicitation handlers into the transport, and uses the real
    /// `StreamableHttpTransport` for `StreamableHttp` configs with OAuth
    /// token management.
    pub async fn connect_with_handlers(
        config: &McpServerConfig,
        store: Option<std::sync::Arc<dyn aivyx_core::StorageBackend>>,
        sampling: Option<std::sync::Arc<dyn SamplingHandler>>,
        elicitation: Option<std::sync::Arc<dyn ElicitationHandler>>,
    ) -> Result<Self> {
        let timeout = std::time::Duration::from_secs(config.timeout_secs);
        let transport: Box<dyn McpTransportLayer> = match &config.transport {
            McpTransportConfig::Stdio { command, args } => {
                let t = StdioTransport::spawn_with_handlers(
                    command,
                    args,
                    &config.env,
                    timeout,
                    sampling.clone(),
                    elicitation.clone(),
                )
                .await?;
                Box::new(t)
            }
            McpTransportConfig::Sse { url, auth } => {
                // Legacy SSE transport — no handler support, just auth.
                let t = if let Some(auth_config) = auth {
                    match &auth_config.method {
                        aivyx_config::McpAuthMethod::Bearer { token_secret_name } => {
                            if let Some(ref s) = store {
                                if let Ok(Some(bytes)) = s.get(token_secret_name) {
                                    let token = String::from_utf8_lossy(&bytes).to_string();
                                    SseTransport::with_auth(url, timeout, format!("Bearer {token}"))
                                } else {
                                    SseTransport::new(url, timeout)
                                }
                            } else {
                                SseTransport::new(url, timeout)
                            }
                        }
                        aivyx_config::McpAuthMethod::OAuth { .. } => {
                            SseTransport::new(url, timeout)
                        }
                    }
                } else {
                    SseTransport::new(url, timeout)
                };
                Box::new(t)
            }
            McpTransportConfig::StreamableHttp { url, auth } => {
                let t = if let Some(auth_config) = auth {
                    match &auth_config.method {
                        aivyx_config::McpAuthMethod::Bearer { token_secret_name } => {
                            // Resolve token from store.
                            let auth_header = if let Some(ref s) = store {
                                if let Ok(Some(bytes)) = s.get(token_secret_name) {
                                    Some(format!("Bearer {}", String::from_utf8_lossy(&bytes)))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if let Some(header) = auth_header {
                                StreamableHttpTransport::with_static_auth(
                                    url,
                                    timeout,
                                    header,
                                    sampling.clone(),
                                    elicitation.clone(),
                                )
                            } else {
                                StreamableHttpTransport::with_handlers(
                                    url,
                                    timeout,
                                    sampling.clone(),
                                    elicitation.clone(),
                                )
                            }
                        }
                        aivyx_config::McpAuthMethod::OAuth {
                            client_id, scopes, ..
                        } => {
                            // Create OAuth token manager for automatic refresh.
                            if let Some(ref s) = store {
                                let oauth = crate::auth::McpOAuthClient::new(
                                    client_id,
                                    scopes.clone(),
                                    "", // Discovery will populate endpoints
                                    "", // Discovery will populate endpoints
                                );
                                let tm = std::sync::Arc::new(OAuthTokenManager::new(
                                    oauth,
                                    &config.name,
                                    s.clone(),
                                ));
                                StreamableHttpTransport::with_auth(
                                    url,
                                    timeout,
                                    tm,
                                    sampling.clone(),
                                    elicitation.clone(),
                                )
                            } else {
                                StreamableHttpTransport::with_handlers(
                                    url,
                                    timeout,
                                    sampling.clone(),
                                    elicitation.clone(),
                                )
                            }
                        }
                    }
                } else {
                    StreamableHttpTransport::with_handlers(
                        url,
                        timeout,
                        sampling.clone(),
                        elicitation.clone(),
                    )
                };
                Box::new(t)
            }
        };

        Ok(Self {
            transport,
            server_name: config.name.clone(),
            next_id: AtomicU64::new(1),
        })
    }

    /// Send the MCP `initialize` handshake.
    ///
    /// This must be called after [`connect`](McpClient::connect) before any
    /// other MCP methods. Sends the `initialize` request followed by the
    /// `notifications/initialized` notification.
    pub async fn initialize(&self) -> Result<InitializeResult> {
        let params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "aivyx",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let request = JsonRpcRequest::new(self.next_request_id(), "initialize", Some(params));
        let response = self.transport.send(&request).await?;
        let result_value = response.into_result()?;

        let init_result: InitializeResult = serde_json::from_value(result_value)
            .map_err(|e| AivyxError::Other(format!("failed to parse initialize result: {e}")))?;

        // Send the initialized notification to complete the handshake.
        let notification = JsonRpcRequest::notification("notifications/initialized", None);
        self.transport.notify(&notification).await?;

        tracing::info!(
            "MCP '{}' initialized (server: {} v{}, protocol: {})",
            self.server_name,
            init_result.server_info.name,
            init_result.server_info.version.as_deref().unwrap_or("?"),
            init_result.protocol_version,
        );

        Ok(init_result)
    }

    /// Discover available tools from the MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let request = JsonRpcRequest::new(
            self.next_request_id(),
            "tools/list",
            Some(serde_json::json!({})),
        );
        let response = self.transport.send(&request).await?;
        let result = response.into_result()?;

        let tools_value = result
            .get("tools")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![]));

        let tools: Vec<McpToolDef> = serde_json::from_value(tools_value)
            .map_err(|e| AivyxError::Other(format!("failed to parse tools/list result: {e}")))?;

        tracing::debug!(
            "MCP '{}' discovered {} tools",
            self.server_name,
            tools.len()
        );

        Ok(tools)
    }

    /// Call a tool on the MCP server.
    ///
    /// Returns the tool's response as a JSON value. If the tool returns an
    /// error (`isError: true`), this is reflected in the returned value, not
    /// as a Rust error.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let request = JsonRpcRequest::new(self.next_request_id(), "tools/call", Some(params));
        let response = self.transport.send(&request).await?;
        let result = response.into_result()?;

        // Try to parse as CallToolResult for structured access.
        match serde_json::from_value::<CallToolResult>(result.clone()) {
            Ok(call_result) => {
                // Concatenate text content items.
                let text: String = call_result
                    .content
                    .iter()
                    .filter_map(|c| c.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");

                if call_result.is_error {
                    Ok(serde_json::json!({
                        "error": true,
                        "content": text,
                    }))
                } else {
                    Ok(serde_json::json!({
                        "content": text,
                    }))
                }
            }
            // If it doesn't match CallToolResult, return raw.
            Err(_) => Ok(result),
        }
    }

    /// Gracefully shut down the MCP connection.
    pub async fn shutdown(&self) -> Result<()> {
        self.transport.shutdown().await
    }

    /// Get the status of an MCP task (async tool execution).
    ///
    /// Sends a `tasks/get` request to query the current state of a
    /// previously started asynchronous task.
    pub async fn get_task_status(&self, task_id: &str) -> Result<McpTaskStatus> {
        let params = serde_json::json!({ "taskId": task_id });
        let request = JsonRpcRequest::new(self.next_request_id(), "tasks/get", Some(params));
        let response = self.transport.send(&request).await?;
        let result = response.into_result()?;
        serde_json::from_value(result)
            .map_err(|e| AivyxError::Other(format!("failed to parse task status: {e}")))
    }

    /// Cancel an MCP task.
    pub async fn cancel_task(&self, task_id: &str) -> Result<()> {
        let params = serde_json::json!({ "taskId": task_id });
        let request = JsonRpcRequest::new(self.next_request_id(), "tasks/cancel", Some(params));
        let response = self.transport.send(&request).await?;
        response.into_result()?;
        Ok(())
    }

    /// Get the server name.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Generate the next monotonically increasing request ID.
    fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;
    use crate::transport::McpTransportLayer;
    use std::sync::Mutex as StdMutex;

    /// Mock transport that returns predefined responses.
    struct MockTransport {
        responses: StdMutex<Vec<JsonRpcResponse>>,
    }

    impl MockTransport {
        fn new(responses: Vec<JsonRpcResponse>) -> Self {
            Self {
                responses: StdMutex::new(responses),
            }
        }
    }

    #[async_trait::async_trait]
    impl McpTransportLayer for MockTransport {
        async fn send(&self, _request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            let mut resps = self.responses.lock().unwrap();
            if resps.is_empty() {
                Err(AivyxError::Other("no more mock responses".into()))
            } else {
                Ok(resps.remove(0))
            }
        }

        async fn notify(&self, _request: &JsonRpcRequest) -> Result<()> {
            Ok(())
        }

        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn initialize_handshake() {
        let init_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {
                    "name": "test-server",
                    "version": "1.0"
                }
            })),
            error: None,
        };

        let transport = MockTransport::new(vec![init_response]);
        let client = McpClient::from_transport(Box::new(transport), "test");
        let result = client.initialize().await;
        assert!(result.is_ok());
        let init = result.unwrap();
        assert_eq!(init.server_info.name, "test-server");
    }

    #[tokio::test]
    async fn list_tools_returns_definitions() {
        let tools_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echoes input",
                        "inputSchema": {"type": "object"}
                    },
                    {
                        "name": "add",
                        "description": "Adds numbers",
                        "inputSchema": {"type": "object"}
                    }
                ]
            })),
            error: None,
        };

        let transport = MockTransport::new(vec![tools_response]);
        let client = McpClient::from_transport(Box::new(transport), "test");
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[1].name, "add");
    }

    #[tokio::test]
    async fn call_tool_returns_content() {
        let call_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "hello world"}],
                "isError": false
            })),
            error: None,
        };

        let transport = MockTransport::new(vec![call_response]);
        let client = McpClient::from_transport(Box::new(transport), "test");
        let result = client
            .call_tool("echo", serde_json::json!({"message": "hello"}))
            .await
            .unwrap();
        assert_eq!(result["content"], "hello world");
    }

    #[tokio::test]
    async fn call_tool_error_response() {
        let error_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "something went wrong"}],
                "isError": true
            })),
            error: None,
        };

        let transport = MockTransport::new(vec![error_response]);
        let client = McpClient::from_transport(Box::new(transport), "test");
        let result = client
            .call_tool("broken", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result["error"], true);
    }

    #[tokio::test]
    async fn get_task_status_returns_state() {
        let task_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "taskId": "task-42",
                "state": "working",
                "progress": 0.3
            })),
            error: None,
        };

        let transport = MockTransport::new(vec![task_response]);
        let client = McpClient::from_transport(Box::new(transport), "test");
        let status = client.get_task_status("task-42").await.unwrap();
        assert_eq!(status.task_id, "task-42");
        assert_eq!(status.state, crate::protocol::McpTaskState::Working);
        assert_eq!(status.progress, Some(0.3));
    }

    #[tokio::test]
    async fn cancel_task_sends_request() {
        let cancel_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({})),
            error: None,
        };

        let transport = MockTransport::new(vec![cancel_response]);
        let client = McpClient::from_transport(Box::new(transport), "test");
        let result = client.cancel_task("task-42").await;
        assert!(result.is_ok());
    }
}
