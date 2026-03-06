//! Plugin management tools for discovering, installing, and removing plugins.
//!
//! Plugins are MCP-based tool packs registered in the system configuration.
//! [`PluginListTool`] lists installed plugins, [`PluginInstallTool`] installs
//! new ones, and [`PluginRemoveTool`] removes them.

use async_trait::async_trait;
use tracing::warn;

use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_config::{AivyxConfig, AivyxDirs, McpServerConfig, McpTransport};
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};

// ---------------------------------------------------------------------------
// PluginListTool
// ---------------------------------------------------------------------------

/// Tool that lists all installed plugins from the system configuration.
pub struct PluginListTool {
    id: ToolId,
    dirs: AivyxDirs,
}

impl PluginListTool {
    /// Create a new plugin listing tool.
    pub fn new(dirs: AivyxDirs) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
        }
    }
}

#[async_trait]
impl Tool for PluginListTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "plugin_list"
    }

    fn description(&self) -> &str {
        "List all installed plugins (MCP-based tool packs) with their name, version, description, and status."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("plugin".into()))
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let config = AivyxConfig::load(self.dirs.config_path())?;
        let plugins: Vec<serde_json::Value> = config
            .plugins
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "version": p.version,
                    "description": p.description,
                    "author": p.author,
                    "enabled": p.enabled,
                    "source": serde_json::to_value(&p.source).unwrap_or_default(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "total": plugins.len(),
            "plugins": plugins
        }))
    }
}

// ---------------------------------------------------------------------------
// PluginInstallTool
// ---------------------------------------------------------------------------

/// Tool that installs a new plugin (MCP-based tool pack).
///
/// Creates an `McpServerConfig` from the provided command and registers
/// it as a `PluginEntry` in the system configuration.
pub struct PluginInstallTool {
    id: ToolId,
    dirs: AivyxDirs,
    audit_log: Option<AuditLog>,
}

impl PluginInstallTool {
    /// Create a new plugin installation tool.
    pub fn new(dirs: AivyxDirs, audit_log: Option<AuditLog>) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
            audit_log,
        }
    }

    fn audit(&self, event: AuditEvent) {
        if let Some(log) = &self.audit_log
            && let Err(e) = log.append(event)
        {
            warn!("Failed to write audit event: {e}");
        }
    }
}

#[async_trait]
impl Tool for PluginInstallTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "plugin_install"
    }

    fn description(&self) -> &str {
        "Install a plugin (MCP-based tool pack) by specifying a command to run. The plugin will be registered in the system configuration."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique name for this plugin"
                },
                "command": {
                    "type": "string",
                    "description": "Command to execute (e.g., 'npx', 'python')"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Arguments to pass to the command"
                },
                "description": {
                    "type": "string",
                    "description": "Human-readable description of what this plugin provides"
                },
                "version": {
                    "type": "string",
                    "description": "Version string (e.g., '1.0.0')"
                }
            },
            "required": ["name", "command", "description"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("plugin".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("plugin_install: missing 'name'".into()))?;
        let command = input["command"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("plugin_install: missing 'command'".into()))?;
        let description = input["description"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("plugin_install: missing 'description'".into()))?;
        let version = input["version"].as_str().unwrap_or("0.1.0");
        let args: Vec<String> = input["args"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Validate name
        if name.is_empty() || name.len() > 64 || name.contains('/') || name.contains('\\') {
            return Err(AivyxError::Agent(
                "plugin_install: invalid plugin name".into(),
            ));
        }

        let mut config = AivyxConfig::load(self.dirs.config_path())?;

        // Check for duplicates
        if config.find_plugin(name).is_some() {
            return Err(AivyxError::Agent(format!(
                "plugin '{name}' is already installed"
            )));
        }

        let mcp_config = McpServerConfig {
            name: name.to_string(),
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args,
            },
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        };

        let entry = aivyx_config::PluginEntry {
            name: name.to_string(),
            version: version.to_string(),
            description: description.to_string(),
            author: input["author"].as_str().map(String::from),
            source: aivyx_config::PluginSource::Local {
                path: command.to_string(),
            },
            mcp_config,
            installed_at: chrono::Utc::now(),
            enabled: true,
        };

        config.add_plugin(entry);
        config.save(self.dirs.config_path())?;

        self.audit(AuditEvent::PluginInstalled {
            plugin_name: name.to_string(),
            source: command.to_string(),
        });

        Ok(serde_json::json!({
            "status": "installed",
            "name": name,
            "version": version
        }))
    }
}

// ---------------------------------------------------------------------------
// PluginRemoveTool
// ---------------------------------------------------------------------------

/// Tool that removes an installed plugin from the system configuration.
///
/// This deregisters the plugin but does not delete any files on disk.
pub struct PluginRemoveTool {
    id: ToolId,
    dirs: AivyxDirs,
    audit_log: Option<AuditLog>,
}

impl PluginRemoveTool {
    /// Create a new plugin removal tool.
    pub fn new(dirs: AivyxDirs, audit_log: Option<AuditLog>) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
            audit_log,
        }
    }

    fn audit(&self, event: AuditEvent) {
        if let Some(log) = &self.audit_log
            && let Err(e) = log.append(event)
        {
            warn!("Failed to write audit event: {e}");
        }
    }
}

#[async_trait]
impl Tool for PluginRemoveTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "plugin_remove"
    }

    fn description(&self) -> &str {
        "Remove an installed plugin by name. This deregisters it from the system configuration but does not delete files."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the plugin to remove"
                }
            },
            "required": ["name"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("plugin".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("plugin_remove: missing 'name'".into()))?;

        let mut config = AivyxConfig::load(self.dirs.config_path())?;

        if config.find_plugin(name).is_none() {
            return Err(AivyxError::Agent(format!(
                "plugin '{name}' is not installed"
            )));
        }

        config.remove_plugin(name);
        config.save(self.dirs.config_path())?;

        self.audit(AuditEvent::PluginRemoved {
            plugin_name: name.to_string(),
        });

        Ok(serde_json::json!({
            "status": "removed",
            "name": name
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_list_tool_schema() {
        let dirs = AivyxDirs::new("/tmp/test-aivyx");
        let tool = PluginListTool::new(dirs);
        assert_eq!(tool.name(), "plugin_list");
        let schema = tool.input_schema();
        assert!(schema["type"].as_str() == Some("object"));
    }

    #[test]
    fn plugin_install_tool_schema() {
        let dirs = AivyxDirs::new("/tmp/test-aivyx");
        let tool = PluginInstallTool::new(dirs, None);
        assert_eq!(tool.name(), "plugin_install");
        let schema = tool.input_schema();
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["command"].is_object());
        assert!(schema["properties"]["description"].is_object());
    }

    #[test]
    fn plugin_remove_tool_schema() {
        let dirs = AivyxDirs::new("/tmp/test-aivyx");
        let tool = PluginRemoveTool::new(dirs, None);
        assert_eq!(tool.name(), "plugin_remove");
        let schema = tool.input_schema();
        assert!(schema["properties"]["name"].is_object());
    }
}
