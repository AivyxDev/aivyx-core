//! Plugin registry types for MCP-based tool packs.
//!
//! A [`PluginEntry`] wraps an [`McpServerConfig`]
//! with metadata (name, version, description, author) for the plugin
//! marketplace system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::mcp::McpServerConfig;

/// A registered plugin (MCP tool pack) in the system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    /// Unique name for the plugin.
    pub name: String,
    /// Semantic version string (e.g., "1.0.0").
    pub version: String,
    /// Human-readable description of what this plugin provides.
    pub description: String,
    /// Optional author name or organization.
    #[serde(default)]
    pub author: Option<String>,
    /// Where the plugin was installed from.
    pub source: PluginSource,
    /// MCP server configuration for connecting to this plugin.
    pub mcp_config: McpServerConfig,
    /// Timestamp when the plugin was installed.
    pub installed_at: DateTime<Utc>,
    /// Whether this plugin is currently active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// Source from which a plugin was installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginSource {
    /// Installed from a local command or path.
    #[serde(rename = "local")]
    Local {
        /// Path or command used to install.
        path: String,
    },
    /// Installed from a remote registry.
    #[serde(rename = "registry")]
    Registry {
        /// URL of the registry entry.
        url: String,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::mcp::McpTransport;

    #[test]
    fn plugin_entry_serde_roundtrip() {
        let entry = PluginEntry {
            name: "code-review".into(),
            version: "1.0.0".into(),
            description: "AI-powered code review tool".into(),
            author: Some("aivyx".into()),
            source: PluginSource::Local {
                path: "/usr/local/bin/code-review-mcp".into(),
            },
            mcp_config: McpServerConfig {
                name: "code-review".into(),
                transport: McpTransport::Stdio {
                    command: "code-review-mcp".into(),
                    args: vec![],
                },
                env: HashMap::new(),
                timeout_secs: 30,
                allowed_tools: None,
                blocked_tools: None,
            },
            installed_at: Utc::now(),
            enabled: true,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let restored: PluginEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "code-review");
        assert_eq!(restored.version, "1.0.0");
        assert!(restored.enabled);
    }

    #[test]
    fn plugin_source_variants() {
        let local = PluginSource::Local {
            path: "/usr/bin/tool".into(),
        };
        let json = serde_json::to_string(&local).unwrap();
        assert!(json.contains("\"type\":\"local\""));

        let registry = PluginSource::Registry {
            url: "https://plugins.aivyx.dev/code-review".into(),
        };
        let json = serde_json::to_string(&registry).unwrap();
        assert!(json.contains("\"type\":\"registry\""));

        let restored: PluginSource = serde_json::from_str(&json).unwrap();
        if let PluginSource::Registry { url } = restored {
            assert!(url.contains("aivyx.dev"));
        } else {
            panic!("wrong variant");
        }
    }
}
