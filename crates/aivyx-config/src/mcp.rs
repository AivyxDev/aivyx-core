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
    /// Whitelist: only these tool names are registered from this server.
    /// If `None`, all discovered tools are allowed (unless `blocked_tools` is set).
    /// Cannot be set together with `blocked_tools`.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Blacklist: these tool names are excluded from registration.
    /// If `None`, no tools are blocked (unless `allowed_tools` is set).
    /// Cannot be set together with `allowed_tools`.
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
    /// Maximum reconnection attempts before giving up. Default: 3.
    #[serde(default = "default_max_reconnect_attempts")]
    pub max_reconnect_attempts: u32,
    /// Base delay in milliseconds for exponential backoff between reconnection
    /// attempts. Actual delay is `base * 2^(attempt-1)`. Default: 1000ms.
    #[serde(default = "default_reconnect_backoff_ms")]
    pub reconnect_backoff_ms: u64,
}

/// Default MCP operation timeout: 30 seconds.
fn default_timeout_secs() -> u64 {
    30
}

fn default_max_reconnect_attempts() -> u32 {
    3
}

fn default_reconnect_backoff_ms() -> u64 {
    1000
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

impl McpServerConfig {
    /// Validate the configuration. Returns an error if both `allowed_tools`
    /// and `blocked_tools` are set (ambiguous intent).
    pub fn validate(&self) -> aivyx_core::Result<()> {
        if self.allowed_tools.is_some() && self.blocked_tools.is_some() {
            return Err(aivyx_core::AivyxError::Config(format!(
                "MCP server '{}': cannot set both allowed_tools and blocked_tools",
                self.name
            )));
        }
        Ok(())
    }

    /// Check if a tool name passes the allow/block filter.
    ///
    /// - If `allowed_tools` is set, only listed names pass.
    /// - If `blocked_tools` is set, all names except listed ones pass.
    /// - If neither is set, all names pass (default behavior).
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if let Some(allowed) = &self.allowed_tools {
            return allowed.iter().any(|a| a == tool_name);
        }
        if let Some(blocked) = &self.blocked_tools {
            return !blocked.iter().any(|b| b == tool_name);
        }
        true
    }
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
            allowed_tools: None,
            blocked_tools: None,
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1000,
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
            allowed_tools: None,
            blocked_tools: None,
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1000,
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
        if let McpTransport::Sse {
            auth: Some(auth), ..
        } = &config.transport
        {
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
        if let McpTransport::Sse {
            auth: Some(auth), ..
        } = &config.transport
        {
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

    #[test]
    fn is_tool_allowed_with_allowlist() {
        let config = McpServerConfig {
            name: "test".into(),
            transport: McpTransport::Stdio {
                command: "echo".into(),
                args: vec![],
            },
            env: HashMap::new(),
            timeout_secs: 30,
            allowed_tools: Some(vec!["echo".into(), "read".into()]),
            blocked_tools: None,
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1000,
        };
        assert!(config.is_tool_allowed("echo"));
        assert!(config.is_tool_allowed("read"));
        assert!(!config.is_tool_allowed("dangerous_tool"));
    }

    #[test]
    fn is_tool_allowed_with_blocklist() {
        let config = McpServerConfig {
            name: "test".into(),
            transport: McpTransport::Stdio {
                command: "echo".into(),
                args: vec![],
            },
            env: HashMap::new(),
            timeout_secs: 30,
            allowed_tools: None,
            blocked_tools: Some(vec!["dangerous".into()]),
        };
        assert!(config.is_tool_allowed("echo"));
        assert!(config.is_tool_allowed("read"));
        assert!(!config.is_tool_allowed("dangerous"));
    }

    #[test]
    fn is_tool_allowed_neither_set() {
        let config = McpServerConfig {
            name: "test".into(),
            transport: McpTransport::Stdio {
                command: "echo".into(),
                args: vec![],
            },
            env: HashMap::new(),
            timeout_secs: 30,
            allowed_tools: None,
            blocked_tools: None,
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1000,
        };
        assert!(config.is_tool_allowed("anything"));
    }

    #[test]
    fn validate_rejects_both_allow_and_block() {
        let config = McpServerConfig {
            name: "bad".into(),
            transport: McpTransport::Stdio {
                command: "echo".into(),
                args: vec![],
            },
            env: HashMap::new(),
            timeout_secs: 30,
            allowed_tools: Some(vec!["a".into()]),
            blocked_tools: Some(vec!["b".into()]),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_accepts_only_allow() {
        let config = McpServerConfig {
            name: "ok".into(),
            transport: McpTransport::Stdio {
                command: "echo".into(),
                args: vec![],
            },
            env: HashMap::new(),
            timeout_secs: 30,
            allowed_tools: Some(vec!["a".into()]),
            blocked_tools: None,
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1000,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn tool_filter_fields_backward_compatible() {
        // Old configs without allowed_tools/blocked_tools should still parse
        let toml_str = r#"
            name = "legacy"
            [transport]
            type = "stdio"
            command = "echo"
        "#;
        let config: McpServerConfig = toml::from_str(toml_str).unwrap();
        assert!(config.allowed_tools.is_none());
        assert!(config.blocked_tools.is_none());
        assert!(config.is_tool_allowed("any_tool"));
    }

    #[test]
    fn tool_filter_fields_toml_roundtrip() {
        let toml_str = r#"
            name = "filtered"
            allowed_tools = ["echo", "search"]
            [transport]
            type = "stdio"
            command = "npx"
        "#;
        let config: McpServerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.allowed_tools,
            Some(vec!["echo".into(), "search".into()])
        );
        assert!(config.blocked_tools.is_none());

        // Roundtrip through TOML
        let serialized = toml::to_string_pretty(&config).unwrap();
        let restored: McpServerConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.allowed_tools, config.allowed_tools);
    }
}
