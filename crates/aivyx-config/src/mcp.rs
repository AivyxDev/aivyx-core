//! MCP (Model Context Protocol) server configuration.
//!
//! Defines the connection parameters for MCP servers that provide
//! external tools to the agent via JSON-RPC 2.0.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for a single MCP server connection.
///
/// Each MCP server provides one or more tools that the agent can discover
/// and invoke at runtime. Servers communicate via either stdio (child
/// process) or HTTP+SSE transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Human-readable name for this server (used in logging and capability scopes).
    pub name: String,
    /// Transport configuration — how to connect to the server.
    pub transport: McpTransport,
    /// Environment variables to pass to stdio child processes.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Timeout in seconds for MCP operations (connect, init, tool calls).
    /// Defaults to 30 seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

/// Default MCP operation timeout: 30 seconds.
fn default_timeout_secs() -> u64 {
    30
}

/// Transport type for connecting to an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpTransport {
    /// Stdio transport: spawn a child process and communicate via stdin/stdout.
    #[serde(rename = "stdio")]
    Stdio {
        /// Command to execute (e.g., "npx", "python").
        command: String,
        /// Arguments to pass to the command.
        #[serde(default)]
        args: Vec<String>,
    },
    /// SSE transport: connect to an HTTP server endpoint.
    #[serde(rename = "sse")]
    Sse {
        /// URL of the MCP server's SSE endpoint.
        url: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_timeout_serde_default() {
        let toml_str = r#"
            name = "no-timeout"
            [transport]
            type = "sse"
            url = "http://localhost:3001/sse"
        "#;
        let config: McpServerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.timeout_secs, 30); // default

        let toml_with_timeout = r#"
            name = "custom-timeout"
            timeout_secs = 120
            [transport]
            type = "sse"
            url = "http://localhost:3001/sse"
        "#;
        let config2: McpServerConfig = toml::from_str(toml_with_timeout).unwrap();
        assert_eq!(config2.timeout_secs, 120);
    }

    #[test]
    fn stdio_config_roundtrip() {
        let config = McpServerConfig {
            name: "test-server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-everything".into(),
                ],
            },
            env: HashMap::from([("NODE_ENV".into(), "production".into())]),
            timeout_secs: 30,
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let restored: McpServerConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(restored.name, "test-server");
        assert!(matches!(restored.transport, McpTransport::Stdio { .. }));
    }

    #[test]
    fn sse_config_roundtrip() {
        let config = McpServerConfig {
            name: "remote-tools".into(),
            transport: McpTransport::Sse {
                url: "http://localhost:3001/sse".into(),
            },
            env: HashMap::new(),
            timeout_secs: 60,
        };

        let json = serde_json::to_string(&config).unwrap();
        let restored: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "remote-tools");
        if let McpTransport::Sse { url } = &restored.transport {
            assert_eq!(url, "http://localhost:3001/sse");
        } else {
            panic!("wrong transport variant");
        }
    }
}
