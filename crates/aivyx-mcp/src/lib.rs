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
//!     └─ McpProxyTool ──→ McpClient ──→ Transport (stdio | SSE)
//!                              │
//!                         ToolResultCache
//! ```
//!
//! Each MCP server connection produces a set of [`McpProxyTool`] instances
//! that implement the [`aivyx_core::Tool`] trait. These are registered in
//! the agent's tool registry alongside built-in tools — the agent doesn't
//! need to know whether a tool is local or remote.
//!
//! # Usage
//!
//! ```rust,no_run
//! # async fn example() -> aivyx_core::Result<()> {
//! use aivyx_mcp::{McpClient, McpProxyTool, ToolResultCache};
//! use aivyx_config::McpServerConfig;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! // Connect to an MCP server.
//! # let config: McpServerConfig = todo!();
//! let client = Arc::new(McpClient::connect(&config).await?);
//! client.initialize().await?;
//!
//! // Discover tools.
//! let tools = client.list_tools().await?;
//!
//! // Wrap each as a proxy tool for the agent's registry.
//! let cache = Arc::new(ToolResultCache::new(Duration::from_secs(300)));
//! for tool_def in tools {
//!     let proxy = McpProxyTool::new(tool_def, client.clone(), "my-server", Some(cache.clone()));
//!     // registry.register(Box::new(proxy));
//! }
//! # Ok(())
//! # }
//! ```

pub mod cache;
pub mod client;
pub mod protocol;
pub mod proxy;
pub mod transport;

pub use cache::ToolResultCache;
pub use client::McpClient;
pub use protocol::McpToolDef;
pub use proxy::McpProxyTool;
pub use transport::McpTransportLayer;
