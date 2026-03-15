//! Configuration management for the aivyx framework.
//!
//! Handles the `~/.aivyx/` directory layout, TOML-based configuration with
//! dotted-key access, LLM provider selection, and autonomy policy settings.

pub mod autonomy_policy;
pub mod cache;
pub mod channel;
pub mod config;
pub mod dirs;
pub mod embedding;
pub mod heartbeat;
pub mod mcp;
pub mod memory;
pub mod nexus;
pub mod plugin;
pub mod project;
pub mod provider;
pub mod schedule;
pub mod server;
pub mod skill;
pub mod smtp;
pub mod speech;

pub use autonomy_policy::{AutonomyPolicy, CircuitBreakerPolicy};
pub use cache::CacheConfig;
pub use channel::{ChannelConfig, ChannelPlatform};
pub use config::{
    AivyxConfig, BackupConfig, BillingConfig, DefaultQuotas, GroupRoleMappingConfig,
    OidcProviderConfig, SsoConfig, TenantsConfig, TriggerConfig,
};
pub use dirs::AivyxDirs;
pub use embedding::EmbeddingConfig;
pub use heartbeat::HeartbeatConfig;
pub use mcp::{McpAuthConfig, McpAuthMethod, McpServerConfig, McpTransport};
pub use memory::MemoryConfig;
pub use nexus::NexusConfig;
pub use plugin::{PluginEntry, PluginSource};
pub use project::ProjectConfig;
pub use provider::{ModelPricing, ProviderConfig};
pub use schedule::{ScheduleEntry, validate_cron};
pub use server::ServerConfig;
pub use skill::{LoadedSkill, SkillManifest, SkillSummary, discover_skills, load_skill};
pub use smtp::SmtpConfig;
pub use speech::{SpeechConfig, SpeechProvider, TtsConfig, TtsProvider};
