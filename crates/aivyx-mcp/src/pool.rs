//! MCP server connection pool — manages the lifecycle of active MCP server connections.
//!
//! The [`McpServerPool`] tracks all connected MCP servers and their clients,
//! enabling graceful shutdown and health monitoring. Without this, stdio child
//! processes spawned by MCP servers would become orphaned when the agent shuts down.
//!
//! # Architecture
//!
//! ```text
//! Agent
//!   └─ McpServerPool
//!        ├─ "fs-server"   → Arc<McpClient> (Connected)
//!        ├─ "web-search"  → Arc<McpClient> (Connected)
//!        └─ "code-review" → Arc<McpClient> (Disconnected)
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use aivyx_config::McpServerConfig;
use tokio::sync::RwLock;

use crate::client::McpClient;

/// Health status of an MCP server connection.
#[derive(Debug, Clone)]
pub enum McpServerHealth {
    /// Server is connected and operational.
    Connected,
    /// Server is disconnected.
    Disconnected {
        since: chrono::DateTime<chrono::Utc>,
        reason: String,
    },
    /// Server is being reconnected.
    Reconnecting {
        attempt: u32,
        since: chrono::DateTime<chrono::Utc>,
    },
}

/// Internal entry tracking a single MCP server connection.
struct PoolEntry {
    client: Arc<McpClient>,
    config: McpServerConfig,
    health: McpServerHealth,
}

/// Manages the lifecycle of active MCP server connections.
///
/// Created during MCP tool discovery and stored on the agent for the
/// duration of the session. Call [`shutdown_all`](McpServerPool::shutdown_all)
/// before dropping to ensure clean shutdown of stdio child processes.
pub struct McpServerPool {
    entries: RwLock<HashMap<String, PoolEntry>>,
}

impl McpServerPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Track a connected client in the pool.
    pub async fn insert(&self, name: String, client: Arc<McpClient>, config: McpServerConfig) {
        self.entries.write().await.insert(
            name,
            PoolEntry {
                client,
                config,
                health: McpServerHealth::Connected,
            },
        );
    }

    /// Get a client by server name.
    pub async fn get(&self, name: &str) -> Option<Arc<McpClient>> {
        self.entries
            .read()
            .await
            .get(name)
            .map(|e| e.client.clone())
    }

    /// Get the config for a server by name.
    pub async fn get_config(&self, name: &str) -> Option<McpServerConfig> {
        self.entries
            .read()
            .await
            .get(name)
            .map(|e| e.config.clone())
    }

    /// Remove and return a client from the pool.
    pub async fn remove(&self, name: &str) -> Option<Arc<McpClient>> {
        self.entries.write().await.remove(name).map(|e| e.client)
    }

    /// Shut down all connected servers gracefully.
    pub async fn shutdown_all(&self) {
        let entries: Vec<_> = {
            let mut map = self.entries.write().await;
            map.drain().collect()
        };
        for (name, entry) in entries {
            if let Err(e) = entry.client.shutdown().await {
                tracing::warn!("Failed to shut down MCP server '{}': {e}", name);
            } else {
                tracing::info!("MCP server '{}' shut down", name);
            }
        }
    }

    /// Number of active connections.
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Returns true if there are no connections.
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }

    /// List connected server names.
    pub async fn server_names(&self) -> Vec<String> {
        self.entries.read().await.keys().cloned().collect()
    }

    /// Get health status for all servers.
    pub async fn health(&self) -> HashMap<String, McpServerHealth> {
        self.entries
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.health.clone()))
            .collect()
    }

    /// Update the health status of a server.
    pub async fn set_health(&self, name: &str, health: McpServerHealth) {
        if let Some(entry) = self.entries.write().await.get_mut(name) {
            entry.health = health;
        }
    }

    /// Replace the client for a server (used after reconnection).
    pub async fn replace_client(&self, name: &str, client: Arc<McpClient>) {
        if let Some(entry) = self.entries.write().await.get_mut(name) {
            entry.client = client;
            entry.health = McpServerHealth::Connected;
        }
    }

    /// Attempt to reconnect to a disconnected MCP server with exponential backoff.
    ///
    /// On success, replaces the old client in the pool and returns the new client.
    /// On failure after all attempts, marks the server as [`McpServerHealth::Disconnected`].
    pub async fn reconnect(&self, server_name: &str) -> aivyx_core::Result<Arc<McpClient>> {
        let config = self.get_config(server_name).await.ok_or_else(|| {
            aivyx_core::AivyxError::Other(format!("unknown MCP server: {server_name}"))
        })?;

        let max_attempts = config.max_reconnect_attempts;
        let base_delay = config.reconnect_backoff_ms;

        for attempt in 1..=max_attempts {
            self.set_health(
                server_name,
                McpServerHealth::Reconnecting {
                    attempt,
                    since: chrono::Utc::now(),
                },
            )
            .await;

            tracing::info!(
                "Reconnecting to MCP '{}' (attempt {}/{})",
                server_name,
                attempt,
                max_attempts
            );

            match McpClient::connect(&config).await {
                Ok(client) => {
                    let client = Arc::new(client);
                    match client.initialize().await {
                        Ok(_init_result) => {
                            self.replace_client(server_name, client.clone()).await;
                            tracing::info!("MCP '{}' reconnected successfully", server_name);
                            return Ok(client);
                        }
                        Err(e) => {
                            tracing::warn!("MCP '{}' reconnect init failed: {e}", server_name);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "MCP '{}' reconnect attempt {}/{} failed: {e}",
                        server_name,
                        attempt,
                        max_attempts
                    );
                }
            }

            // Exponential backoff: base * 2^(attempt-1)
            let delay =
                std::time::Duration::from_millis(base_delay.saturating_mul(1u64 << (attempt - 1)));
            tokio::time::sleep(delay).await;
        }

        // Mark as disconnected after all attempts failed.
        self.set_health(
            server_name,
            McpServerHealth::Disconnected {
                since: chrono::Utc::now(),
                reason: format!("reconnection failed after {} attempts", max_attempts),
            },
        )
        .await;

        Err(aivyx_core::AivyxError::Other(format!(
            "MCP '{}' reconnection failed after {} attempts",
            server_name, max_attempts
        )))
    }
}

impl Default for McpServerPool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for McpServerPool {
    fn drop(&mut self) {
        let entries = self.entries.get_mut();
        if !entries.is_empty() {
            tracing::warn!(
                "McpServerPool dropped with {} active connections; \
                 call shutdown_all() before dropping for clean shutdown",
                entries.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
    use crate::transport::McpTransportLayer;
    use aivyx_core::Result;
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;

    struct MockTransport {
        shutdown_called: Arc<StdMutex<bool>>,
    }

    #[async_trait]
    impl McpTransportLayer for MockTransport {
        async fn send(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            Err(aivyx_core::AivyxError::Other("not implemented".into()))
        }
        async fn notify(&self, _req: &JsonRpcRequest) -> Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> Result<()> {
            *self.shutdown_called.lock().unwrap() = true;
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
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1000,
        }
    }

    fn mock_client(name: &str) -> (Arc<McpClient>, Arc<StdMutex<bool>>) {
        let shutdown_flag = Arc::new(StdMutex::new(false));
        let transport = MockTransport {
            shutdown_called: shutdown_flag.clone(),
        };
        let client = Arc::new(McpClient::from_transport(Box::new(transport), name));
        (client, shutdown_flag)
    }

    #[tokio::test]
    async fn pool_insert_and_get() {
        let pool = McpServerPool::new();
        let (client, _) = mock_client("test");

        pool.insert("test".into(), client.clone(), mock_config("test"))
            .await;
        assert_eq!(pool.len().await, 1);
        assert!(!pool.is_empty().await);

        let retrieved = pool.get("test").await;
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn pool_remove() {
        let pool = McpServerPool::new();
        let (client, _) = mock_client("removable");

        pool.insert("removable".into(), client, mock_config("removable"))
            .await;
        assert_eq!(pool.len().await, 1);

        let removed = pool.remove("removable").await;
        assert!(removed.is_some());
        assert!(pool.is_empty().await);
    }

    #[tokio::test]
    async fn pool_server_names() {
        let pool = McpServerPool::new();
        let (c1, _) = mock_client("alpha");
        let (c2, _) = mock_client("beta");

        pool.insert("alpha".into(), c1, mock_config("alpha")).await;
        pool.insert("beta".into(), c2, mock_config("beta")).await;

        let mut names = pool.server_names().await;
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[tokio::test]
    async fn shutdown_all_calls_shutdown_on_clients() {
        let pool = McpServerPool::new();
        let (c1, flag1) = mock_client("srv1");
        let (c2, flag2) = mock_client("srv2");

        pool.insert("srv1".into(), c1, mock_config("srv1")).await;
        pool.insert("srv2".into(), c2, mock_config("srv2")).await;

        pool.shutdown_all().await;

        assert!(*flag1.lock().unwrap(), "srv1 should have been shut down");
        assert!(*flag2.lock().unwrap(), "srv2 should have been shut down");
        assert!(pool.is_empty().await);
    }

    #[tokio::test]
    async fn health_tracking() {
        let pool = McpServerPool::new();
        let (client, _) = mock_client("health-test");

        pool.insert("health-test".into(), client, mock_config("health-test"))
            .await;

        let health = pool.health().await;
        assert!(matches!(
            health.get("health-test"),
            Some(McpServerHealth::Connected)
        ));

        pool.set_health(
            "health-test",
            McpServerHealth::Disconnected {
                since: chrono::Utc::now(),
                reason: "test".into(),
            },
        )
        .await;

        let health = pool.health().await;
        assert!(matches!(
            health.get("health-test"),
            Some(McpServerHealth::Disconnected { .. })
        ));
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let pool = McpServerPool::new();
        assert!(pool.get("ghost").await.is_none());
        assert!(pool.get_config("ghost").await.is_none());
    }
}
