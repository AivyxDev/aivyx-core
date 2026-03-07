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
        /// Optional authentication for the remote MCP server.
        #[serde(default)]
        auth: Option<McpAuthConfig>,
    },
}

/// Authentication configuration for a remote MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAuthConfig {
    /// Authentication method to use.
    pub method: McpAuthMethod,
}

/// Authentication method for MCP server connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpAuthMethod {
    /// OAuth 2.1 with PKCE — discovers metadata from the server's
    /// `/.well-known/oauth-authorization-server` endpoint and performs
    /// the authorization code flow with PKCE.
    #[serde(rename = "oauth")]
    OAuth {
        /// OAuth client ID registered with the MCP server.
        client_id: String,
        /// Requested OAuth scopes.
        #[serde(default)]
        scopes: Vec<String>,
    },
    /// Static Bearer token — uses a pre-configured token stored in the
    /// encrypted secrets store.
    Bearer {
        /// Name of the secret in the encrypted store containing the token.
        token_secret_name: String,
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
                auth: None,
            },
            env: HashMap::new(),
            timeout_secs: 60,
        };

        let json = serde_json::to_string(&config).unwrap();
        let restored: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "remote-tools");
        if let McpTransport::Sse { url, auth } = &restored.transport {
            assert_eq!(url, "http://localhost:3001/sse");
            assert!(auth.is_none());
        } else {
            panic!("wrong transport variant");
        }
    }

    #[test]
    fn sse_with_oauth_config() {
        let toml_str = r#"
            name = "oauth-server"
            [transport]
            type = "sse"
            url = "https://mcp.example.com/sse"
            [transport.auth]
            [transport.auth.method]
            type = "oauth"
            client_id = "my-client-id"
            scopes = ["tools:read", "tools:execute"]
        "#;
        let config: McpServerConfig = toml::from_str(toml_str).unwrap();
        if let McpTransport::Sse { auth: Some(auth), .. } = &config.transport {
            match &auth.method {
                McpAuthMethod::OAuth { client_id, scopes } => {
                    assert_eq!(client_id, "my-client-id");
                    assert_eq!(scopes.len(), 2);
                }
                _ => panic!("expected OAuth method"),
            }
        } else {
            panic!("expected Sse with auth");
        }
    }

    #[test]
    fn sse_with_bearer_config() {
        let toml_str = r#"
            name = "bearer-server"
            [transport]
            type = "sse"
            url = "https://mcp.example.com/sse"
            [transport.auth]
            [transport.auth.method]
            type = "bearer"
            token_secret_name = "mcp-api-key"
        "#;
        let config: McpServerConfig = toml::from_str(toml_str).unwrap();
        if let McpTransport::Sse { auth: Some(auth), .. } = &config.transport {
            match &auth.method {
                McpAuthMethod::Bearer { token_secret_name } => {
                    assert_eq!(token_secret_name, "mcp-api-key");
                }
                _ => panic!("expected Bearer method"),
            }
        } else {
            panic!("expected Sse with auth");
        }
    }

    #[test]
    fn sse_without_auth_backwards_compatible() {
        // Existing configs without auth field should still work
        let toml_str = r#"
            name = "no-auth"
            [transport]
            type = "sse"
            url = "http://localhost:3001/sse"
        "#;
        let config: McpServerConfig = toml::from_str(toml_str).unwrap();
        if let McpTransport::Sse { auth, .. } = &config.transport {
            assert!(auth.is_none());
        } else {
            panic!("expected Sse transport");
        }
    }
}
