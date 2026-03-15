//! Streamable HTTP transport for the Nov 2025 MCP specification.
//!
//! Implements the MCP Streamable HTTP transport: a single `/mcp` endpoint
//! supporting both direct JSON responses and SSE streaming in the response
//! body. This replaces the deprecated SSE transport.
//!
//! Key behaviors:
//! - Requests are sent as HTTP POST with `application/json` body
//! - Responses may be `application/json` (direct) or `text/event-stream` (SSE)
//! - SSE streams may contain server-initiated requests (sampling, elicitation)
//! - Session management via `Mcp-Session-Id` header
//! - Shutdown sends `DELETE` to the endpoint with the session ID

use std::sync::Arc;
use std::time::Duration;

use aivyx_core::{AivyxError, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::RwLock;

use crate::auth::OAuthTokenManager;
use crate::client::{ElicitationHandler, SamplingHandler};
use crate::protocol::{
    ElicitationRequest, IncomingJsonRpcRequest, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse,
    SamplingRequest,
};
use crate::transport::McpTransportLayer;

/// MCP Streamable HTTP transport.
///
/// Sends JSON-RPC requests as HTTP POST and handles both direct JSON
/// responses and SSE-streamed responses. Supports bidirectional communication
/// by dispatching server-initiated sampling and elicitation requests to
/// registered handlers.
pub struct StreamableHttpTransport {
    /// HTTP client instance.
    client: reqwest::Client,
    /// MCP server endpoint URL (typically ending in `/mcp`).
    url: String,
    /// Session ID from the server, sent as `Mcp-Session-Id` header.
    session_id: Arc<RwLock<Option<String>>>,
    /// OAuth token manager for automatic token refresh.
    token_manager: Option<Arc<OAuthTokenManager>>,
    /// Static authorization header value (e.g., "Bearer <token>").
    static_auth: Option<String>,
    /// Handler for server-initiated sampling requests.
    sampling_handler: Option<Arc<dyn SamplingHandler>>,
    /// Handler for server-initiated elicitation requests.
    elicitation_handler: Option<Arc<dyn ElicitationHandler>>,
}

impl StreamableHttpTransport {
    /// Create a transport with no authentication or handlers.
    pub fn new(url: &str, timeout: Duration) -> Self {
        Self::build(url, timeout, None, None, None, None)
    }

    /// Create a transport with sampling and elicitation handlers.
    pub fn with_handlers(
        url: &str,
        timeout: Duration,
        sampling: Option<Arc<dyn SamplingHandler>>,
        elicitation: Option<Arc<dyn ElicitationHandler>>,
    ) -> Self {
        Self::build(url, timeout, None, None, sampling, elicitation)
    }

    /// Create a transport with OAuth token management and handlers.
    pub fn with_auth(
        url: &str,
        timeout: Duration,
        token_manager: Arc<OAuthTokenManager>,
        sampling: Option<Arc<dyn SamplingHandler>>,
        elicitation: Option<Arc<dyn ElicitationHandler>>,
    ) -> Self {
        Self::build(
            url,
            timeout,
            Some(token_manager),
            None,
            sampling,
            elicitation,
        )
    }

    /// Create a transport with a static authorization header.
    pub fn with_static_auth(
        url: &str,
        timeout: Duration,
        auth_header: String,
        sampling: Option<Arc<dyn SamplingHandler>>,
        elicitation: Option<Arc<dyn ElicitationHandler>>,
    ) -> Self {
        Self::build(url, timeout, None, Some(auth_header), sampling, elicitation)
    }

    fn build(
        url: &str,
        timeout: Duration,
        token_manager: Option<Arc<OAuthTokenManager>>,
        static_auth: Option<String>,
        sampling: Option<Arc<dyn SamplingHandler>>,
        elicitation: Option<Arc<dyn ElicitationHandler>>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();

        Self {
            client,
            url: url.to_string(),
            session_id: Arc::new(RwLock::new(None)),
            token_manager,
            static_auth,
            sampling_handler: sampling,
            elicitation_handler: elicitation,
        }
    }

    /// Build an HTTP POST request with auth and session headers.
    async fn build_request(&self, request: &JsonRpcRequest) -> Result<reqwest::RequestBuilder> {
        let mut builder = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        // Inject auth header.
        if let Some(ref tm) = self.token_manager {
            let token = tm.get_valid_token().await?;
            builder = builder.header("Authorization", format!("Bearer {token}"));
        } else if let Some(ref auth) = self.static_auth {
            builder = builder.header("Authorization", auth);
        }

        // Include session ID if we have one.
        if let Some(ref sid) = *self.session_id.read().await {
            builder = builder.header("Mcp-Session-Id", sid);
        }

        builder = builder.json(request);
        Ok(builder)
    }

    /// Capture the `Mcp-Session-Id` header from a response.
    async fn capture_session_id(&self, response: &reqwest::Response) {
        if let Some(sid) = response.headers().get("mcp-session-id")
            && let Ok(sid_str) = sid.to_str()
        {
            let mut session = self.session_id.write().await;
            *session = Some(sid_str.to_string());
        }
    }

    /// Parse an SSE stream from the response body.
    ///
    /// Reads SSE events, looking for the JSON-RPC response that matches
    /// our request. Also dispatches server-initiated requests (sampling,
    /// elicitation) to registered handlers.
    async fn read_sse_stream(
        &self,
        response: reqwest::Response,
        request_id: Option<u64>,
    ) -> Result<JsonRpcResponse> {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut data_lines = Vec::new();
        let mut our_response: Option<JsonRpcResponse> = None;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| AivyxError::Http(format!("SSE stream read error: {e}")))?;

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete lines from the buffer.
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    // Blank line = end of SSE event. Process accumulated data.
                    if !data_lines.is_empty() {
                        let data = data_lines.join("\n");
                        data_lines.clear();

                        if data == "[DONE]" {
                            continue;
                        }

                        match serde_json::from_str::<JsonRpcMessage>(&data) {
                            Ok(JsonRpcMessage::Response(resp)) => {
                                // Check if this is the response to our request.
                                if resp.id == request_id {
                                    our_response = Some(resp);
                                }
                                // Other responses are dropped (shouldn't happen in practice).
                            }
                            Ok(JsonRpcMessage::Request(req)) => {
                                // Server-initiated request — handle asynchronously.
                                self.handle_server_request(req).await;
                            }
                            Err(e) => {
                                tracing::debug!(
                                    "SSE event parse error: {e} — data: {}",
                                    data.chars().take(200).collect::<String>()
                                );
                            }
                        }
                    }
                } else if let Some(data_content) = line.strip_prefix("data:") {
                    data_lines.push(data_content.trim_start().to_string());
                }
                // Ignore other SSE fields (event:, id:, retry:, comments).
            }

            // If we got our response, we can stop reading.
            if our_response.is_some() {
                break;
            }
        }

        // Process any remaining data in the buffer (no trailing newline).
        if our_response.is_none() && !data_lines.is_empty() {
            let data = data_lines.join("\n");
            if data != "[DONE]"
                && let Ok(JsonRpcMessage::Response(resp)) = serde_json::from_str(&data)
                && resp.id == request_id
            {
                our_response = Some(resp);
            }
        }

        our_response
            .ok_or_else(|| AivyxError::Http("SSE stream ended without a matching response".into()))
    }

    /// Handle a server-initiated request received in an SSE stream.
    ///
    /// Dispatches sampling and elicitation requests to registered handlers,
    /// then POSTs the response back to the server.
    async fn handle_server_request(&self, request: IncomingJsonRpcRequest) {
        let Some(request_id) = request.id else {
            tracing::debug!(
                "Server-initiated notification in SSE stream: {}",
                request.method
            );
            return;
        };

        let response_value = match request.method.as_str() {
            "sampling/createMessage" => self.handle_sampling(request_id, request.params).await,
            "elicitation/create" => self.handle_elicitation(request_id, request.params).await,
            method => {
                tracing::debug!("Unknown server-initiated method in SSE: {method}");
                Some(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32601,
                        "message": format!("method not supported: {method}")
                    }
                }))
            }
        };

        // POST the response back to the server.
        if let Some(resp) = response_value
            && let Err(e) = self.post_response(resp).await
        {
            tracing::warn!("Failed to send response to server-initiated request: {e}");
        }
    }

    /// Handle a sampling/createMessage request from the server.
    async fn handle_sampling(
        &self,
        request_id: u64,
        params: Option<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let Some(ref handler) = self.sampling_handler else {
            tracing::warn!("Server sent sampling/createMessage but no handler configured");
            return Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "sampling not supported"}
            }));
        };

        let sampling_req = match params {
            Some(p) => match serde_json::from_value::<SamplingRequest>(p) {
                Ok(r) => r,
                Err(e) => {
                    return Some(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {"code": -32602, "message": format!("invalid params: {e}")}
                    }));
                }
            },
            None => {
                return Some(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32602, "message": "missing params"}
                }));
            }
        };

        match handler.create_message(sampling_req).await {
            Ok(response) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": response,
            })),
            Err(e) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32000, "message": e.to_string()}
            })),
        }
    }

    /// Handle an elicitation/create request from the server.
    async fn handle_elicitation(
        &self,
        request_id: u64,
        params: Option<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let Some(ref handler) = self.elicitation_handler else {
            tracing::warn!("Server sent elicitation/create but no handler configured");
            return Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "elicitation not supported"}
            }));
        };

        let elicitation_req = match params {
            Some(p) => match serde_json::from_value::<ElicitationRequest>(p) {
                Ok(r) => r,
                Err(e) => {
                    return Some(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {"code": -32602, "message": format!("invalid params: {e}")}
                    }));
                }
            },
            None => {
                return Some(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32602, "message": "missing params"}
                }));
            }
        };

        match handler.elicit(elicitation_req).await {
            Ok(response) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": response,
            })),
            Err(e) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32000, "message": e.to_string()}
            })),
        }
    }

    /// POST a JSON-RPC response back to the server (for server-initiated requests).
    async fn post_response(&self, response: serde_json::Value) -> Result<()> {
        let mut builder = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json");

        if let Some(ref tm) = self.token_manager {
            let token = tm.get_valid_token().await?;
            builder = builder.header("Authorization", format!("Bearer {token}"));
        } else if let Some(ref auth) = self.static_auth {
            builder = builder.header("Authorization", auth);
        }

        if let Some(ref sid) = *self.session_id.read().await {
            builder = builder.header("Mcp-Session-Id", sid);
        }

        let resp = builder
            .json(&response)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("POST response failed: {e}")))?;

        if !resp.status().is_success() {
            tracing::warn!("Server rejected response POST: HTTP {}", resp.status());
        }

        Ok(())
    }
}

#[async_trait]
impl McpTransportLayer for StreamableHttpTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let builder = self.build_request(request).await?;

        let response = builder
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("MCP HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(AivyxError::Http(format!(
                "MCP server returned HTTP {}",
                response.status()
            )));
        }

        // Capture session ID from response headers.
        self.capture_session_id(&response).await;

        // Branch on Content-Type: direct JSON or SSE stream.
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();

        if content_type.contains("text/event-stream") {
            self.read_sse_stream(response, request.id).await
        } else {
            // Direct JSON response.
            let body = response
                .text()
                .await
                .map_err(|e| AivyxError::Http(format!("MCP response body error: {e}")))?;

            serde_json::from_str(&body).map_err(|e| {
                AivyxError::Other(format!(
                    "MCP response parse error: {e} — body: {}",
                    body.chars().take(500).collect::<String>()
                ))
            })
        }
    }

    async fn notify(&self, request: &JsonRpcRequest) -> Result<()> {
        let builder = self.build_request(request).await?;

        let response = builder
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("MCP notification failed: {e}")))?;

        if !response.status().is_success() {
            return Err(AivyxError::Http(format!(
                "MCP server returned HTTP {} for notification",
                response.status()
            )));
        }

        // Capture session ID even from notification responses.
        self.capture_session_id(&response).await;
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let session_id = self.session_id.read().await.clone();
        let Some(sid) = session_id else {
            return Ok(()); // No session to close.
        };

        let mut builder = self.client.delete(&self.url).header("Mcp-Session-Id", &sid);

        if let Some(ref tm) = self.token_manager {
            if let Ok(token) = tm.get_valid_token().await {
                builder = builder.header("Authorization", format!("Bearer {token}"));
            }
        } else if let Some(ref auth) = self.static_auth {
            builder = builder.header("Authorization", auth);
        }

        match builder.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!("MCP session {sid} closed via DELETE");
            }
            Ok(resp) => {
                tracing::debug!(
                    "MCP DELETE returned HTTP {} (session may have expired)",
                    resp.status()
                );
            }
            Err(e) => {
                tracing::debug!("MCP DELETE failed (server may be down): {e}");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;

    #[test]
    fn transport_creation() {
        let transport =
            StreamableHttpTransport::new("http://localhost:3001/mcp", Duration::from_secs(60));
        assert_eq!(transport.url, "http://localhost:3001/mcp");
        assert!(transport.token_manager.is_none());
        assert!(transport.static_auth.is_none());
        assert!(transport.sampling_handler.is_none());
        assert!(transport.elicitation_handler.is_none());
    }

    #[test]
    fn transport_with_static_auth() {
        let transport = StreamableHttpTransport::with_static_auth(
            "http://localhost:3001/mcp",
            Duration::from_secs(60),
            "Bearer test-token".into(),
            None,
            None,
        );
        assert_eq!(transport.static_auth.as_deref(), Some("Bearer test-token"));
    }

    #[tokio::test]
    async fn session_id_starts_none() {
        let transport =
            StreamableHttpTransport::new("http://localhost:3001/mcp", Duration::from_secs(60));
        assert!(transport.session_id.read().await.is_none());
    }

    #[test]
    fn parse_sse_data_line() {
        // Verify our SSE parsing logic by testing the line prefix stripping.
        let line = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}";
        let data = line.strip_prefix("data:").unwrap().trim_start();
        let resp: JsonRpcResponse = serde_json::from_str(data).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
    }

    #[test]
    fn parse_sse_data_no_space() {
        // SSE spec allows `data:content` without a space after the colon.
        let line = "data:{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}";
        let data = line.strip_prefix("data:").unwrap().trim_start();
        let resp: JsonRpcResponse = serde_json::from_str(data).unwrap();
        assert_eq!(resp.id, Some(2));
    }

    #[test]
    fn done_sentinel_is_recognized() {
        let data = "[DONE]";
        assert_eq!(data, "[DONE]");
        // Verify it does NOT parse as JSON-RPC.
        assert!(serde_json::from_str::<JsonRpcMessage>(data).is_err());
    }

    #[tokio::test]
    async fn shutdown_without_session_is_noop() {
        let transport =
            StreamableHttpTransport::new("http://localhost:3001/mcp", Duration::from_secs(60));
        // Should succeed without making any HTTP requests since no session exists.
        let result = transport.shutdown().await;
        assert!(result.is_ok());
    }

    #[test]
    fn transport_with_handlers() {
        use crate::client::AutoDismissElicitationHandler;

        let elicitation = Arc::new(AutoDismissElicitationHandler);
        let transport = StreamableHttpTransport::with_handlers(
            "http://localhost:3001/mcp",
            Duration::from_secs(60),
            None,
            Some(elicitation),
        );
        assert!(transport.elicitation_handler.is_some());
        assert!(transport.sampling_handler.is_none());
    }
}
