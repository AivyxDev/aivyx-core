//! Inbound communication channel configuration.
//!
//! Each [`ChannelConfig`] defines a connection to an external messaging
//! platform (Telegram, Email, Discord, Slack, Matrix). The engine receives
//! messages from these platforms and routes them through the agent turn loop.

use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A configured inbound communication channel.
///
/// Stored as a `[[channels]]` entry in `config.toml`. Each channel connects
/// to an external messaging platform and routes incoming messages through the
/// configured agent profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelConfig {
    /// Unique name for this channel (slug-style, e.g., `"telegram-personal"`).
    pub name: String,
    /// Which messaging platform this channel connects to.
    pub platform: ChannelPlatform,
    /// Agent profile name to handle incoming messages.
    pub agent: String,
    /// Whether this channel is currently active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Platform-specific user identifiers that are allowed to send messages.
    ///
    /// Empty means **deny all** — no messages will be processed.
    /// For Telegram: numeric user IDs. For Email: email addresses.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Platform-specific settings (e.g., `bot_token_ref`, `poll_interval_secs`).
    ///
    /// Secrets are stored by reference (e.g., `bot_token_ref = "tg-bot-token"`)
    /// pointing to a key in the encrypted store, never as plaintext values.
    #[serde(default)]
    pub settings: HashMap<String, String>,
    /// When this channel was configured.
    pub created_at: DateTime<Utc>,
}

/// Supported messaging platforms.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ChannelPlatform {
    /// Telegram Bot API (long-polling or webhook).
    Telegram,
    /// Email via IMAP (receive) and SMTP (send).
    Email,
    /// Discord gateway (WebSocket).
    Discord,
    /// Slack Events API (webhook).
    Slack,
    /// Matrix client-server API (sync polling).
    Matrix,
    /// WhatsApp Business Cloud API (webhook).
    WhatsApp,
}

fn default_true() -> bool {
    true
}

impl ChannelConfig {
    /// Create a new channel configuration with defaults.
    pub fn new(
        name: impl Into<String>,
        platform: ChannelPlatform,
        agent: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            platform,
            agent: agent.into(),
            enabled: true,
            allowed_users: Vec::new(),
            settings: HashMap::new(),
            created_at: Utc::now(),
        }
    }
}

impl fmt::Display for ChannelPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelPlatform::Telegram => write!(f, "telegram"),
            ChannelPlatform::Email => write!(f, "email"),
            ChannelPlatform::Discord => write!(f, "discord"),
            ChannelPlatform::Slack => write!(f, "slack"),
            ChannelPlatform::Matrix => write!(f, "matrix"),
            ChannelPlatform::WhatsApp => write!(f, "whatsapp"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_config_new() {
        let config = ChannelConfig::new("tg-personal", ChannelPlatform::Telegram, "assistant");
        assert_eq!(config.name, "tg-personal");
        assert_eq!(config.platform, ChannelPlatform::Telegram);
        assert_eq!(config.agent, "assistant");
        assert!(config.enabled);
        assert!(config.allowed_users.is_empty());
        assert!(config.settings.is_empty());
    }

    #[test]
    fn channel_config_json_roundtrip() {
        let mut config = ChannelConfig::new("email-work", ChannelPlatform::Email, "assistant");
        config.allowed_users = vec!["user@example.com".into()];
        config
            .settings
            .insert("imap_host".into(), "imap.example.com".into());

        let json = serde_json::to_string(&config).unwrap();
        let parsed: ChannelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "email-work");
        assert_eq!(parsed.platform, ChannelPlatform::Email);
        assert_eq!(parsed.allowed_users, vec!["user@example.com"]);
        assert_eq!(
            parsed.settings.get("imap_host").unwrap(),
            "imap.example.com"
        );
    }

    #[test]
    fn channel_config_toml_roundtrip() {
        let config = ChannelConfig::new("tg-bot", ChannelPlatform::Telegram, "assistant");
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ChannelConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.name, "tg-bot");
        assert_eq!(parsed.platform, ChannelPlatform::Telegram);
        assert!(parsed.enabled);
    }

    #[test]
    fn channel_platform_display() {
        assert_eq!(ChannelPlatform::Telegram.to_string(), "telegram");
        assert_eq!(ChannelPlatform::Email.to_string(), "email");
        assert_eq!(ChannelPlatform::Discord.to_string(), "discord");
        assert_eq!(ChannelPlatform::Slack.to_string(), "slack");
        assert_eq!(ChannelPlatform::Matrix.to_string(), "matrix");
        assert_eq!(ChannelPlatform::WhatsApp.to_string(), "whatsapp");
    }

    #[test]
    fn channel_config_defaults_on_deserialize() {
        let json = r#"{
            "name": "test",
            "platform": "Telegram",
            "agent": "assistant",
            "created_at": "2026-03-04T00:00:00Z"
        }"#;
        let parsed: ChannelConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.enabled); // default_true
        assert!(parsed.allowed_users.is_empty()); // default empty
        assert!(parsed.settings.is_empty()); // default empty
    }
}
