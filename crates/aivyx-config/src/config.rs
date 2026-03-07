use std::collections::HashMap;
use std::path::Path;

use aivyx_core::{AivyxError, Result};
use serde::{Deserialize, Serialize};

use crate::autonomy_policy::AutonomyPolicy;
use crate::channel::ChannelConfig;
use crate::embedding::EmbeddingConfig;
use crate::memory::MemoryConfig;
use crate::plugin::PluginEntry;
use crate::project::ProjectConfig;
use crate::provider::ProviderConfig;
use crate::schedule::ScheduleEntry;
use crate::server::ServerConfig;
use crate::smtp::SmtpConfig;
use crate::speech::SpeechConfig;

/// Top-level aivyx configuration, persisted as TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AivyxConfig {
    /// Which LLM provider to use and how to reach it.
    pub provider: ProviderConfig,
    /// Agent autonomy constraints and rate limits.
    pub autonomy: AutonomyPolicy,
    /// Embedding provider for the memory system.
    /// `None` means memory features use the default (Ollama, nomic-embed-text).
    pub embedding: Option<EmbeddingConfig>,
    /// Named LLM provider configurations.
    /// Agents reference these by name via `provider` in their profile.
    /// If empty or name not found, agents use the top-level `provider` config.
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    /// Memory subsystem configuration (pruning limits).
    #[serde(default)]
    pub memory: MemoryConfig,
    /// HTTP server configuration.
    /// `None` means server features use defaults (127.0.0.1:3000).
    pub server: Option<ServerConfig>,
    /// Registered project directories.
    ///
    /// Projects enable scoped memory recall, contextual prompting, and codebase
    /// navigation. Stored as `[[projects]]` in TOML.
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    /// Scheduled background tasks (cron-driven agent prompts).
    ///
    /// Each entry fires an agent turn on a cron schedule and optionally stores
    /// the result as a notification. Stored as `[[schedules]]` in TOML.
    #[serde(default)]
    pub schedules: Vec<ScheduleEntry>,
    /// Inbound communication channels (Telegram, Email, etc.).
    ///
    /// Each channel connects to an external messaging platform and routes
    /// incoming messages through the configured agent. Stored as
    /// `[[channels]]` in TOML.
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
    /// Installed plugins (MCP-based tool packs).
    ///
    /// Each plugin wraps an `McpServerConfig` with metadata. Stored as
    /// `[[plugins]]` in TOML.
    #[serde(default)]
    pub plugins: Vec<PluginEntry>,
    /// SMTP configuration for outbound email via the `email_send` tool.
    ///
    /// `None` means email sending is disabled.
    pub smtp: Option<SmtpConfig>,
    /// Speech-to-text configuration for voice input.
    ///
    /// `None` means voice features are disabled.
    pub speech: Option<SpeechConfig>,
    /// Federation configuration for cross-instance agent communication.
    ///
    /// Enables Ed25519-authenticated peer-to-peer communication between
    /// separate Aivyx Engine instances. `None` means federation is disabled.
    /// Stored as `[federation]` in TOML.
    #[serde(default)]
    pub federation: Option<aivyx_federation::config::FederationConfig>,
}

impl AivyxConfig {
    /// Load config from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())?;
        toml::from_str(&content).map_err(|e| AivyxError::TomlDe(e.to_string()))
    }

    /// Save config to a TOML file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let content =
            toml::to_string_pretty(self).map_err(|e| AivyxError::TomlSer(e.to_string()))?;
        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }

    /// Resolve a provider config by name.
    ///
    /// Returns the named provider from `providers` if found, otherwise falls
    /// back to the default `provider` config. This enables per-agent provider
    /// selection while remaining backward-compatible.
    pub fn resolve_provider(&self, name: Option<&str>) -> &ProviderConfig {
        match name {
            Some(n) => self.providers.get(n).unwrap_or(&self.provider),
            None => &self.provider,
        }
    }

    /// Find a registered project by name.
    pub fn find_project(&self, name: &str) -> Option<&ProjectConfig> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// Find a registered project whose path is a prefix of the given path.
    ///
    /// Uses longest prefix match — if `/home/user/projects/aivyx` and
    /// `/home/user/projects` are both registered, CWD
    /// `/home/user/projects/aivyx/crates` matches the former.
    pub fn find_project_by_path(&self, path: &Path) -> Option<&ProjectConfig> {
        self.projects
            .iter()
            .filter(|p| path.starts_with(&p.path))
            .max_by_key(|p| p.path.components().count())
    }

    /// Register a new project. Returns an error if a project with the same
    /// name already exists.
    pub fn add_project(&mut self, project: ProjectConfig) -> Result<()> {
        if self.projects.iter().any(|p| p.name == project.name) {
            return Err(AivyxError::Config(format!(
                "project '{}' already registered",
                project.name
            )));
        }
        self.projects.push(project);
        Ok(())
    }

    /// Remove a registered project by name. Returns the removed config, or an
    /// error if not found.
    pub fn remove_project(&mut self, name: &str) -> Result<ProjectConfig> {
        let idx = self
            .projects
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| AivyxError::Config(format!("project '{name}' not found")))?;
        Ok(self.projects.remove(idx))
    }

    /// Find an installed plugin by name.
    pub fn find_plugin(&self, name: &str) -> Option<&PluginEntry> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// Install a new plugin. Appends to the plugins list.
    pub fn add_plugin(&mut self, entry: PluginEntry) {
        self.plugins.push(entry);
    }

    /// Remove a plugin by name. Returns the removed entry, or `None` if not found.
    pub fn remove_plugin(&mut self, name: &str) -> Option<PluginEntry> {
        let idx = self.plugins.iter().position(|p| p.name == name)?;
        Some(self.plugins.remove(idx))
    }

    /// Find a schedule entry by name.
    pub fn find_schedule(&self, name: &str) -> Option<&ScheduleEntry> {
        self.schedules.iter().find(|s| s.name == name)
    }

    /// Find a mutable reference to a schedule entry by name.
    pub fn find_schedule_mut(&mut self, name: &str) -> Option<&mut ScheduleEntry> {
        self.schedules.iter_mut().find(|s| s.name == name)
    }

    /// Add a schedule entry. Returns an error if the name already exists.
    pub fn add_schedule(&mut self, entry: ScheduleEntry) -> Result<()> {
        if self.schedules.iter().any(|s| s.name == entry.name) {
            return Err(AivyxError::Config(format!(
                "schedule '{}' already exists",
                entry.name
            )));
        }
        self.schedules.push(entry);
        Ok(())
    }

    /// Remove a schedule entry by name. Returns the removed entry or an error.
    pub fn remove_schedule(&mut self, name: &str) -> Result<ScheduleEntry> {
        let idx = self
            .schedules
            .iter()
            .position(|s| s.name == name)
            .ok_or_else(|| AivyxError::Config(format!("schedule '{name}' not found")))?;
        Ok(self.schedules.remove(idx))
    }

    /// Find a channel by name.
    pub fn find_channel(&self, name: &str) -> Option<&ChannelConfig> {
        self.channels.iter().find(|c| c.name == name)
    }

    /// Find a mutable reference to a channel by name.
    pub fn find_channel_mut(&mut self, name: &str) -> Option<&mut ChannelConfig> {
        self.channels.iter_mut().find(|c| c.name == name)
    }

    /// Add a channel. Returns an error if the name already exists.
    pub fn add_channel(&mut self, channel: ChannelConfig) -> Result<()> {
        if self.channels.iter().any(|c| c.name == channel.name) {
            return Err(AivyxError::Config(format!(
                "channel '{}' already exists",
                channel.name
            )));
        }
        self.channels.push(channel);
        Ok(())
    }

    /// Remove a channel by name. Returns the removed entry, or an error if not found.
    pub fn remove_channel(&mut self, name: &str) -> Result<ChannelConfig> {
        let idx = self
            .channels
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| AivyxError::Config(format!("channel '{name}' not found")))?;
        Ok(self.channels.remove(idx))
    }

    /// Get a config value by dotted key path (e.g., "autonomy.default_tier").
    pub fn get(&self, key: &str) -> Option<String> {
        let value = toml::to_string(self).ok()?;
        let table: toml::Table = toml::from_str(&value).ok()?;
        resolve_key(&toml::Value::Table(table), key)
    }

    /// Set a config value by dotted key path. Returns an error if the key
    /// doesn't exist or the value can't be parsed for that field.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let mut toml_str = toml::to_string(self).map_err(|e| AivyxError::TomlSer(e.to_string()))?;
        let mut table: toml::Table =
            toml::from_str(&toml_str).map_err(|e| AivyxError::TomlDe(e.to_string()))?;

        set_key(&mut table, key, value)?;

        toml_str = toml::to_string(&table).map_err(|e| AivyxError::TomlSer(e.to_string()))?;
        let updated: AivyxConfig =
            toml::from_str(&toml_str).map_err(|e| AivyxError::TomlDe(e.to_string()))?;

        *self = updated;
        Ok(())
    }
}

fn resolve_key(value: &toml::Value, key: &str) -> Option<String> {
    let parts: Vec<&str> = key.splitn(2, '.').collect();
    match value {
        toml::Value::Table(t) => {
            let child = t.get(parts[0])?;
            if parts.len() == 1 {
                Some(format_value(child))
            } else {
                resolve_key(child, parts[1])
            }
        }
        _ => None,
    }
}

fn format_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn set_key(table: &mut toml::Table, key: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = key.splitn(2, '.').collect();
    if parts.len() == 1 {
        let existing = table
            .get(parts[0])
            .ok_or_else(|| AivyxError::Config(format!("unknown config key: {key}")))?;
        let new_value = parse_as_same_type(existing, value)?;
        table.insert(parts[0].to_string(), new_value);
        Ok(())
    } else {
        let child = table
            .get_mut(parts[0])
            .ok_or_else(|| AivyxError::Config(format!("unknown config section: {}", parts[0])))?;
        match child {
            toml::Value::Table(t) => set_key(t, parts[1], value),
            _ => Err(AivyxError::Config(format!("{} is not a section", parts[0]))),
        }
    }
}

fn parse_as_same_type(existing: &toml::Value, value: &str) -> Result<toml::Value> {
    match existing {
        toml::Value::String(_) => Ok(toml::Value::String(value.to_string())),
        toml::Value::Integer(_) => {
            let i: i64 = value
                .parse()
                .map_err(|_| AivyxError::Config(format!("expected integer, got: {value}")))?;
            Ok(toml::Value::Integer(i))
        }
        toml::Value::Float(_) => {
            let f: f64 = value
                .parse()
                .map_err(|_| AivyxError::Config(format!("expected float, got: {value}")))?;
            Ok(toml::Value::Float(f))
        }
        toml::Value::Boolean(_) => {
            let b: bool = value
                .parse()
                .map_err(|_| AivyxError::Config(format!("expected bool, got: {value}")))?;
            Ok(toml::Value::Boolean(b))
        }
        _ => Ok(toml::Value::String(value.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = AivyxConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let _parsed: AivyxConfig = toml::from_str(&toml_str).unwrap();
    }

    #[test]
    fn save_load_roundtrip() {
        let config = AivyxConfig::default();
        let path = std::env::temp_dir().join(format!("aivyx-cfg-{}.toml", rand::random::<u64>()));
        config.save(&path).unwrap();
        let loaded = AivyxConfig::load(&path).unwrap();
        assert_eq!(loaded.autonomy.default_tier, config.autonomy.default_tier,);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn get_nested_key() {
        let config = AivyxConfig::default();
        let val = config.get("autonomy.max_tool_calls_per_minute");
        assert_eq!(val, Some("60".to_string()));
    }

    #[test]
    fn get_unknown_key_returns_none() {
        let config = AivyxConfig::default();
        assert!(config.get("nonexistent.key").is_none());
    }

    #[test]
    fn set_updates_value() {
        let mut config = AivyxConfig::default();
        config
            .set("autonomy.max_tool_calls_per_minute", "120")
            .unwrap();
        assert_eq!(config.autonomy.max_tool_calls_per_minute, 120);
    }

    #[test]
    fn set_unknown_key_errors() {
        let mut config = AivyxConfig::default();
        assert!(config.set("nonexistent.key", "value").is_err());
    }

    #[test]
    fn resolve_provider_none_returns_default() {
        let config = AivyxConfig::default();
        let resolved = config.resolve_provider(None);
        assert_eq!(resolved.model_name(), config.provider.model_name());
    }

    #[test]
    fn resolve_provider_named() {
        let mut config = AivyxConfig::default();
        config.providers.insert(
            "coding".into(),
            ProviderConfig::Ollama {
                base_url: "http://localhost:11434".into(),
                model: "deepseek-coder-v2".into(),
            },
        );
        let resolved = config.resolve_provider(Some("coding"));
        assert_eq!(resolved.model_name(), "deepseek-coder-v2");
    }

    #[test]
    fn resolve_provider_unknown_falls_back() {
        let config = AivyxConfig::default();
        let resolved = config.resolve_provider(Some("nonexistent"));
        assert_eq!(resolved.model_name(), config.provider.model_name());
    }

    #[test]
    fn find_project_by_name() {
        let mut config = AivyxConfig::default();
        config
            .add_project(ProjectConfig::new("aivyx", "/home/user/aivyx"))
            .unwrap();

        assert!(config.find_project("aivyx").is_some());
        assert_eq!(config.find_project("aivyx").unwrap().name, "aivyx");
        assert!(config.find_project("nonexistent").is_none());
    }

    #[test]
    fn find_project_by_path_longest_prefix() {
        let mut config = AivyxConfig::default();
        config
            .add_project(ProjectConfig::new("projects", "/home/user/projects"))
            .unwrap();
        config
            .add_project(ProjectConfig::new("aivyx", "/home/user/projects/aivyx"))
            .unwrap();

        // CWD inside aivyx → matches aivyx (longer prefix)
        let found = config
            .find_project_by_path(Path::new("/home/user/projects/aivyx/crates"))
            .unwrap();
        assert_eq!(found.name, "aivyx");

        // CWD in projects but not aivyx → matches projects
        let found = config
            .find_project_by_path(Path::new("/home/user/projects/other"))
            .unwrap();
        assert_eq!(found.name, "projects");

        // CWD outside all projects → None
        assert!(config.find_project_by_path(Path::new("/tmp")).is_none());
    }

    #[test]
    fn add_project_name_collision() {
        let mut config = AivyxConfig::default();
        config
            .add_project(ProjectConfig::new("aivyx", "/home/user/aivyx"))
            .unwrap();
        assert!(
            config
                .add_project(ProjectConfig::new("aivyx", "/different/path"))
                .is_err()
        );
    }

    #[test]
    fn remove_project() {
        let mut config = AivyxConfig::default();
        config
            .add_project(ProjectConfig::new("aivyx", "/home/user/aivyx"))
            .unwrap();
        let removed = config.remove_project("aivyx").unwrap();
        assert_eq!(removed.name, "aivyx");
        assert!(config.find_project("aivyx").is_none());
    }

    #[test]
    fn remove_project_not_found() {
        let mut config = AivyxConfig::default();
        assert!(config.remove_project("nonexistent").is_err());
    }

    #[test]
    fn add_schedule_collision() {
        let mut config = AivyxConfig::default();
        config
            .add_schedule(ScheduleEntry::new("daily", "0 7 * * *", "assistant", "Hi"))
            .unwrap();
        assert!(
            config
                .add_schedule(ScheduleEntry::new("daily", "0 8 * * *", "coder", "Hey"))
                .is_err()
        );
    }

    #[test]
    fn remove_schedule_not_found() {
        let mut config = AivyxConfig::default();
        assert!(config.remove_schedule("nonexistent").is_err());
    }

    #[test]
    fn find_schedule_by_name() {
        let mut config = AivyxConfig::default();
        config
            .add_schedule(ScheduleEntry::new("daily", "0 7 * * *", "assistant", "Hi"))
            .unwrap();
        assert!(config.find_schedule("daily").is_some());
        assert_eq!(config.find_schedule("daily").unwrap().cron, "0 7 * * *");
        assert!(config.find_schedule("nonexistent").is_none());
    }

    #[test]
    fn config_with_schedules_toml_roundtrip() {
        let mut config = AivyxConfig::default();
        config
            .add_schedule(ScheduleEntry::new(
                "morning",
                "0 7 * * *",
                "assistant",
                "Good morning!",
            ))
            .unwrap();

        let path =
            std::env::temp_dir().join(format!("aivyx-cfg-sched-{}.toml", rand::random::<u64>()));
        config.save(&path).unwrap();
        let loaded = AivyxConfig::load(&path).unwrap();
        assert_eq!(loaded.schedules.len(), 1);
        assert_eq!(loaded.schedules[0].name, "morning");
        assert_eq!(loaded.schedules[0].cron, "0 7 * * *");
        assert!(loaded.schedules[0].notify);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn config_with_projects_toml_roundtrip() {
        let mut config = AivyxConfig::default();
        let mut proj = ProjectConfig::new("aivyx", "/home/user/aivyx");
        proj.language = Some("Rust".into());
        proj.description = Some("AI framework".into());
        config.add_project(proj).unwrap();

        let path =
            std::env::temp_dir().join(format!("aivyx-cfg-proj-{}.toml", rand::random::<u64>()));
        config.save(&path).unwrap();
        let loaded = AivyxConfig::load(&path).unwrap();
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "aivyx");
        assert_eq!(loaded.projects[0].language.as_deref(), Some("Rust"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn add_channel_collision() {
        use crate::channel::{ChannelConfig, ChannelPlatform};

        let mut config = AivyxConfig::default();
        config
            .add_channel(ChannelConfig::new(
                "tg-personal",
                ChannelPlatform::Telegram,
                "assistant",
            ))
            .unwrap();
        assert!(
            config
                .add_channel(ChannelConfig::new(
                    "tg-personal",
                    ChannelPlatform::Email,
                    "coder"
                ))
                .is_err()
        );
    }

    #[test]
    fn remove_channel_not_found() {
        let mut config = AivyxConfig::default();
        assert!(config.remove_channel("nonexistent").is_err());
    }

    #[test]
    fn find_channel_by_name() {
        use crate::channel::{ChannelConfig, ChannelPlatform};

        let mut config = AivyxConfig::default();
        config
            .add_channel(ChannelConfig::new(
                "tg-personal",
                ChannelPlatform::Telegram,
                "assistant",
            ))
            .unwrap();
        assert!(config.find_channel("tg-personal").is_some());
        assert_eq!(
            config.find_channel("tg-personal").unwrap().platform,
            ChannelPlatform::Telegram
        );
        assert!(config.find_channel("nonexistent").is_none());
    }

    #[test]
    fn config_with_channels_toml_roundtrip() {
        use crate::channel::{ChannelConfig, ChannelPlatform};

        let mut config = AivyxConfig::default();
        let mut ch = ChannelConfig::new("tg-bot", ChannelPlatform::Telegram, "assistant");
        ch.allowed_users = vec!["123456".into()];
        ch.settings
            .insert("bot_token_ref".into(), "tg-bot-token".into());
        config.add_channel(ch).unwrap();

        let path =
            std::env::temp_dir().join(format!("aivyx-cfg-chan-{}.toml", rand::random::<u64>()));
        config.save(&path).unwrap();
        let loaded = AivyxConfig::load(&path).unwrap();
        assert_eq!(loaded.channels.len(), 1);
        assert_eq!(loaded.channels[0].name, "tg-bot");
        assert_eq!(loaded.channels[0].platform, ChannelPlatform::Telegram);
        assert_eq!(loaded.channels[0].allowed_users, vec!["123456"]);
        assert_eq!(
            loaded.channels[0].settings.get("bot_token_ref").unwrap(),
            "tg-bot-token"
        );
        std::fs::remove_file(&path).ok();
    }
}
