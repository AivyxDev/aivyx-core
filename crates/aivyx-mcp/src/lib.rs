//! MCP (Model Context Protocol) client for the aivyx agent framework.
//!
//! This crate implements the client side of the Model Context Protocol,
//! enabling aivyx agents to discover and invoke tools provided by external
//! MCP servers. Communication happens via JSON-RPC 2.0 over stdio (child
//! processes) or HTTP+SSE transports.
//!
//! # Architecture
//!
//! ```text
//! Agent ToolRegistry
//!     └─ McpProxyTool ──→ McpServerPool ──→ McpClient ──→ Transport (stdio | SSE)
//!                              │                   │
//!                         ToolResultCache    reconnect on failure
//! ```
//!
//! Each MCP server connection produces a set of [`McpProxyTool`] instances
//! that implement the [`aivyx_core::Tool`] trait. These are registered in
//! the agent's tool registry alongside built-in tools — the agent doesn't
//! need to know whether a tool is local or remote.
//!
//! The [`McpServerPool`] manages the lifecycle of all connected MCP servers,
//! enabling graceful shutdown and automatic reconnection on failure.
//!
//! # Usage
//!
//! ```rust,no_run
//! # async fn example() -> aivyx_core::Result<()> {
//! use aivyx_mcp::{McpClient, McpProxyTool, McpServerPool, ToolResultCache};
//! use aivyx_config::McpServerConfig;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! // Connect to an MCP server.
//! # let config: McpServerConfig = todo!();
//! let client = Arc::new(McpClient::connect(&config).await?);
//! client.initialize().await?;
//!
//! // Track in pool for lifecycle management.
//! let pool = Arc::new(McpServerPool::new());
//! pool.insert("my-server".into(), client.clone(), config.clone()).await;
//!
//! // Discover tools.
//! let tools = client.list_tools().await?;
//!
//! // Wrap each as a proxy tool for the agent's registry.
//! let cache = Arc::new(ToolResultCache::new(Duration::from_secs(300)));
//! for tool_def in tools {
//!     let proxy = McpProxyTool::new(tool_def, pool.clone(), "my-server", Some(cache.clone()));
//!     // registry.register(Box::new(proxy));
//! }
//!
//! // Graceful shutdown when done.
//! pool.shutdown_all().await;
//! # Ok(())
//! # }
//! ```

pub mod auth;
pub mod cache;
pub mod client;
pub mod pool;
pub mod protocol;
pub mod proxy;
pub mod transport;

pub use auth::{McpOAuthClient, OAuthMetadata, OAuthTokens, PkceChallenge};
pub use cache::ToolResultCache;
pub use client::{AutoDismissElicitationHandler, ElicitationHandler, McpClient, SamplingHandler};
pub use pool::{McpServerHealth, McpServerPool};
pub use protocol::{
    ElicitationAction, ElicitationRequest, ElicitationResponse, McpToolDef, SamplingContent,
    SamplingMessage, SamplingRequest, SamplingResponse,
};
pub use proxy::{McpProxyTool, McpToolCallEvent, McpToolCallObserver};
pub use transport::McpTransportLayer;
