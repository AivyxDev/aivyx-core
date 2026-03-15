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
use crate::pool::McpServerPool;
use crate::protocol::McpToolDef;

/// Events emitted during MCP tool call execution for observability.
#[derive(Debug, Clone)]
pub enum McpToolCallEvent {
    /// A tool call is about to be executed.
    Started {
        server_name: String,
        tool_name: String,
    },
    /// A tool call completed successfully.
    Completed {
        server_name: String,
        tool_name: String,
        duration_ms: u64,
    },
    /// A tool call failed.
    Failed {
        server_name: String,
        tool_name: String,
        error: String,
    },
}

/// Observer callback for MCP tool call lifecycle events.
///
/// Injected by the agent layer to bridge MCP tool execution with the
/// audit log without creating a direct dependency from `aivyx-mcp`
/// to `aivyx-audit`.
pub type McpToolCallObserver = Arc<dyn Fn(McpToolCallEvent) + Send + Sync>;

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
    /// Server connection pool for client lookup and reconnection.
    pool: Arc<McpServerPool>,
    /// Server name (used for capability scope `mcp:<server_name>` and pool lookup).
    server_name: String,
    /// Optional result cache for expensive tool calls.
    cache: Option<Arc<ToolResultCache>>,
    /// Optional observer for audit/observability of tool calls.
    observer: Option<McpToolCallObserver>,
}

impl McpProxyTool {
    /// Create a new proxy tool for an MCP tool definition.
    ///
    /// The `pool` must already contain a client for `server_name`.
    pub fn new(
        tool_def: McpToolDef,
        pool: Arc<McpServerPool>,
        server_name: &str,
        cache: Option<Arc<ToolResultCache>>,
    ) -> Self {
        Self {
            id: ToolId::new(),
            tool_def,
            pool,
            server_name: server_name.to_string(),
            cache,
            observer: None,
        }
    }

    /// Create a new proxy tool with an observer for audit/observability.
    pub fn with_observer(
        tool_def: McpToolDef,
        pool: Arc<McpServerPool>,
        server_name: &str,
        cache: Option<Arc<ToolResultCache>>,
        observer: McpToolCallObserver,
    ) -> Self {
        Self {
            id: ToolId::new(),
            tool_def,
            pool,
            server_name: server_name.to_string(),
            cache,
            observer: Some(observer),
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

        // Notify observer of start.
        if let Some(obs) = &self.observer {
            obs(McpToolCallEvent::Started {
                server_name: self.server_name.clone(),
                tool_name: self.name().to_string(),
            });
        }

        let start = std::time::Instant::now();

        // Get client from pool.
        let client = self.pool.get(&self.server_name).await.ok_or_else(|| {
            aivyx_core::AivyxError::Other(format!("MCP server '{}' not in pool", self.server_name))
        })?;

        // Forward to MCP server; retry once after reconnection on failure.
        match client.call_tool(self.name(), input.clone()).await {
            Ok(result) => {
                if let Some(obs) = &self.observer {
                    obs(McpToolCallEvent::Completed {
                        server_name: self.server_name.clone(),
                        tool_name: self.name().to_string(),
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                }

                // Cache the result.
                if let Some(cache) = &self.cache {
                    let key = ToolResultCache::cache_key(self.name(), &input);
                    cache.insert(&key, result.clone());
                }

                Ok(result)
            }
            Err(original_err) => {
                // Attempt reconnection and retry once.
                tracing::warn!(
                    "MCP tool '{}' call failed, attempting reconnect: {original_err}",
                    self.name()
                );

                match self.pool.reconnect(&self.server_name).await {
                    Ok(new_client) => {
                        match new_client.call_tool(self.name(), input.clone()).await {
                            Ok(result) => {
                                if let Some(obs) = &self.observer {
                                    obs(McpToolCallEvent::Completed {
                                        server_name: self.server_name.clone(),
                                        tool_name: self.name().to_string(),
                                        duration_ms: start.elapsed().as_millis() as u64,
                                    });
                                }
                                if let Some(cache) = &self.cache {
                                    let key = ToolResultCache::cache_key(self.name(), &input);
                                    cache.insert(&key, result.clone());
                                }
                                Ok(result)
                            }
                            Err(retry_err) => {
                                if let Some(obs) = &self.observer {
                                    obs(McpToolCallEvent::Failed {
                                        server_name: self.server_name.clone(),
                                        tool_name: self.name().to_string(),
                                        error: retry_err.to_string(),
                                    });
                                }
                                Err(retry_err)
                            }
                        }
                    }
                    Err(_reconnect_err) => {
                        if let Some(obs) = &self.observer {
                            obs(McpToolCallEvent::Failed {
                                server_name: self.server_name.clone(),
                                tool_name: self.name().to_string(),
                                error: original_err.to_string(),
                            });
                        }
                        Err(original_err)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::McpClient;
    use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
    use crate::transport::McpTransportLayer;
    use aivyx_config::McpServerConfig;
    use std::collections::HashMap;
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

    fn mock_config(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            transport: aivyx_config::McpTransport::Stdio {
                command: "echo".into(),
                args: vec![],
            },
            env: HashMap::new(),
            timeout_secs: 30,
            allowed_tools: None,
            blocked_tools: None,
            max_reconnect_attempts: 1,
            reconnect_backoff_ms: 1,
        }
    }

    /// Helper: create a pool with a mock client inserted.
    async fn pool_with_mock(name: &str, responses: Vec<JsonRpcResponse>) -> Arc<McpServerPool> {
        let transport = MockTransport {
            responses: StdMutex::new(responses),
        };
        let client = Arc::new(McpClient::from_transport(Box::new(transport), name));
        let pool = Arc::new(McpServerPool::new());
        pool.insert(name.into(), client, mock_config(name)).await;
        pool
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

        let pool = pool_with_mock("test-server", vec![call_response]).await;

        let tool_def = McpToolDef {
            name: "echo".into(),
            description: Some("Echoes input".into()),
            input_schema: serde_json::json!({"type": "object"}),
        };

        let proxy = McpProxyTool::new(tool_def, pool, "test-server", None);

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
        let call_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "result"}],
                "isError": false
            })),
            error: None,
        };

        let pool = pool_with_mock("test", vec![call_response]).await;
        let cache = Arc::new(ToolResultCache::new(Duration::from_secs(300)));

        let tool_def = McpToolDef {
            name: "search".into(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        };

        let proxy = McpProxyTool::new(tool_def, pool, "test", Some(cache));

        let input = serde_json::json!({"query": "rust"});

        // First call goes to mock transport.
        let r1 = proxy.execute(input.clone()).await.unwrap();
        assert_eq!(r1["content"], "result");

        // Second call should hit cache (mock has no more responses).
        let r2 = proxy.execute(input).await.unwrap();
        assert_eq!(r2["content"], "result");
    }

    #[tokio::test]
    async fn proxy_tool_name_contains_server() {
        let pool = pool_with_mock("my-server", vec![]).await;

        let tool_def = McpToolDef {
            name: "my_tool".into(),
            description: Some("A test tool".into()),
            input_schema: serde_json::json!({"type": "object"}),
        };

        let proxy = McpProxyTool::new(tool_def, pool, "my-server", None);

        assert_eq!(proxy.name(), "my_tool");

        let scope = proxy.required_scope().unwrap();
        if let CapabilityScope::Custom(ref s) = scope {
            assert!(s.contains("my-server"), "scope should include server name");
            assert_eq!(s, "mcp:my-server");
        } else {
            panic!("expected Custom scope");
        }
    }

    #[tokio::test]
    async fn observer_receives_events_on_success() {
        let call_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}],
                "isError": false
            })),
            error: None,
        };

        let pool = pool_with_mock("obs-test", vec![call_response]).await;

        let tool_def = McpToolDef {
            name: "echo".into(),
            description: Some("Echo tool".into()),
            input_schema: serde_json::json!({"type": "object"}),
        };

        let events: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(vec![]));
        let events_clone = events.clone();
        let observer: McpToolCallObserver = Arc::new(move |event| {
            let label = match &event {
                McpToolCallEvent::Started { .. } => "started",
                McpToolCallEvent::Completed { .. } => "completed",
                McpToolCallEvent::Failed { .. } => "failed",
            };
            events_clone.lock().unwrap().push(label.into());
        });

        let proxy = McpProxyTool::with_observer(tool_def, pool, "obs-test", None, observer);

        proxy
            .execute(serde_json::json!({"msg": "hi"}))
            .await
            .unwrap();

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0], "started");
        assert_eq!(captured[1], "completed");
    }

    #[tokio::test]
    async fn observer_receives_failed_on_error() {
        // No responses — transport will return an error.
        // Pool has no config to reconnect (reconnect will fail → Failed event).
        let pool = pool_with_mock("fail-test", vec![]).await;

        let tool_def = McpToolDef {
            name: "bad_tool".into(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        };

        let events: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(vec![]));
        let events_clone = events.clone();
        let observer: McpToolCallObserver = Arc::new(move |event| {
            let label = match &event {
                McpToolCallEvent::Started { .. } => "started",
                McpToolCallEvent::Completed { .. } => "completed",
                McpToolCallEvent::Failed { .. } => "failed",
            };
            events_clone.lock().unwrap().push(label.into());
        });

        let proxy = McpProxyTool::with_observer(tool_def, pool, "fail-test", None, observer);

        let result = proxy.execute(serde_json::json!({})).await;
        assert!(result.is_err());

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0], "started");
        assert_eq!(captured[1], "failed");
    }

    #[tokio::test]
    async fn no_observer_still_works() {
        let call_response = JsonRpcResponse {
            id: Some(1),
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}],
                "isError": false
            })),
            error: None,
        };

        let pool = pool_with_mock("no-obs", vec![call_response]).await;

        let tool_def = McpToolDef {
            name: "echo".into(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        };

        let proxy = McpProxyTool::new(tool_def, pool, "no-obs", None);
        let result = proxy.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["content"], "ok");
    }
}
