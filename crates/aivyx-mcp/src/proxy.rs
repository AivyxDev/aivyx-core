//! MCP proxy tool — wraps a remote MCP tool as a native [`Tool`] implementation.
//!
//! Each MCP tool discovered via `tools/list` is wrapped in an [`McpProxyTool`]
//! instance that can be registered in the agent's [`ToolRegistry`](aivyx_core::ToolRegistry). When the
//! agent calls the tool, the proxy forwards the invocation to the MCP server
//! via [`McpClient::call_tool`].

use std::sync::Arc;

use aivyx_core::{CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;
use serde_json::Value;

use crate::cache::ToolResultCache;
use crate::client::McpClient;
use crate::protocol::McpToolDef;

/// Proxy tool that wraps a remote MCP tool definition.
///
/// Implements the [`Tool`] trait so it can be registered in the agent's
/// tool registry alongside built-in tools. Tool execution is forwarded
/// to the MCP server, with optional result caching.
pub struct McpProxyTool {
    /// Unique tool ID for registry lookup.
    id: ToolId,
    /// MCP tool definition (name, description, schema).
    tool_def: McpToolDef,
    /// Shared MCP client for making `tools/call` requests.
    client: Arc<McpClient>,
    /// Server name (used for capability scope `mcp:<server_name>`).
    server_name: String,
    /// Optional result cache for expensive tool calls.
    cache: Option<Arc<ToolResultCache>>,
}

impl McpProxyTool {
    /// Create a new proxy tool for an MCP tool definition.
    pub fn new(
        tool_def: McpToolDef,
        client: Arc<McpClient>,
        server_name: &str,
        cache: Option<Arc<ToolResultCache>>,
    ) -> Self {
        Self {
            id: ToolId::new(),
            tool_def,
            client,
            server_name: server_name.to_string(),
            cache,
        }
    }
}

#[async_trait]
impl Tool for McpProxyTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        &self.tool_def.name
    }

    fn description(&self) -> &str {
        self.tool_def
            .description
            .as_deref()
            .unwrap_or("MCP tool (no description)")
    }

    fn input_schema(&self) -> Value {
        self.tool_def.input_schema.clone()
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom(format!("mcp:{}", self.server_name)))
    }

    async fn execute(&self, input: Value) -> Result<Value> {
        // Check cache first.
        if let Some(cache) = &self.cache {
            let key = ToolResultCache::cache_key(self.name(), &input);
            if let Some(cached) = cache.get(&key) {
                tracing::debug!("MCP tool '{}' cache hit", self.name());
                return Ok(cached);
            }
        }

        // Forward to MCP server.
        let result = self.client.call_tool(self.name(), input.clone()).await?;

        // Cache the result.
        if let Some(cache) = &self.cache {
            let key = ToolResultCache::cache_key(self.name(), &input);
            cache.insert(&key, result.clone());
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
    use crate::transport::McpTransportLayer;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    struct MockTransport {
        responses: StdMutex<Vec<JsonRpcResponse>>,
    }

    #[async_trait]
    impl McpTransportLayer for MockTransport {
        async fn send(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            let mut resps = self.responses.lock().unwrap();
            if resps.is_empty() {
                Err(aivyx_core::AivyxError::Other("no mock responses".into()))
            } else {
                Ok(resps.remove(0))
            }
        }
        async fn notify(&self, _req: &JsonRpcRequest) -> Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn proxy_tool_implements_tool_trait() {
        let call_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "echoed"}],
                "isError": false
            })),
            error: None,
        };

        let transport = MockTransport {
            responses: StdMutex::new(vec![call_response]),
        };
        let client = Arc::new(McpClient::from_transport(
            Box::new(transport),
            "test-server",
        ));

        let tool_def = McpToolDef {
            name: "echo".into(),
            description: Some("Echoes input".into()),
            input_schema: serde_json::json!({"type": "object"}),
        };

        let proxy = McpProxyTool::new(tool_def, client, "test-server", None);

        assert_eq!(proxy.name(), "echo");
        assert_eq!(proxy.description(), "Echoes input");
        assert_eq!(
            proxy.required_scope(),
            Some(CapabilityScope::Custom("mcp:test-server".into()))
        );

        let result = proxy
            .execute(serde_json::json!({"message": "hello"}))
            .await
            .unwrap();
        assert_eq!(result["content"], "echoed");
    }

    #[tokio::test]
    async fn proxy_tool_uses_cache() {
        // First call returns from "server", second from cache.
        let call_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "result"}],
                "isError": false
            })),
            error: None,
        };

        let transport = MockTransport {
            responses: StdMutex::new(vec![call_response]),
            // Only one response — second call must come from cache.
        };
        let client = Arc::new(McpClient::from_transport(Box::new(transport), "test"));

        let cache = Arc::new(ToolResultCache::new(Duration::from_secs(300)));

        let tool_def = McpToolDef {
            name: "search".into(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        };

        let proxy = McpProxyTool::new(tool_def, client, "test", Some(cache));

        let input = serde_json::json!({"query": "rust"});

        // First call goes to mock transport.
        let r1 = proxy.execute(input.clone()).await.unwrap();
        assert_eq!(r1["content"], "result");

        // Second call should hit cache (mock has no more responses).
        let r2 = proxy.execute(input).await.unwrap();
        assert_eq!(r2["content"], "result");
    }

    #[test]
    fn proxy_tool_name_contains_server() {
        let transport = MockTransport {
            responses: StdMutex::new(vec![]),
        };
        let client = Arc::new(McpClient::from_transport(Box::new(transport), "my-server"));

        let tool_def = McpToolDef {
            name: "my_tool".into(),
            description: Some("A test tool".into()),
            input_schema: serde_json::json!({"type": "object"}),
        };

        let proxy = McpProxyTool::new(tool_def, client, "my-server", None);

        // The tool name comes from the tool definition.
        assert_eq!(proxy.name(), "my_tool");

        // The required scope should contain the server name as mcp:<server>.
        let scope = proxy.required_scope().unwrap();
        if let CapabilityScope::Custom(ref s) = scope {
            assert!(s.contains("my-server"), "scope should include server name");
            assert_eq!(s, "mcp:my-server");
        } else {
            panic!("expected Custom scope");
        }
    }
}
