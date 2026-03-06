//! SMTP configuration for outbound email.

use serde::{Deserialize, Serialize};

/// SMTP server configuration for the `email_send` tool.
///
/// Secrets (the SMTP password) are stored by reference — `password_ref` names a
/// key in `EncryptedStore`, following the same pattern as `api_key_ref` and
/// `bot_token_ref` elsewhere in the config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpConfig {
    /// SMTP server hostname (e.g. `"smtp.gmail.com"`).
    pub host: String,

    /// SMTP server port. Defaults to 587 (STARTTLS).
    #[serde(default = "default_smtp_port")]
    pub port: u16,

    /// SMTP username (typically the full email address).
    pub username: String,

    /// Key name in `EncryptedStore` that holds the SMTP password.
    pub password_ref: String,

    /// Sender email address for the `From` header.
    pub from_address: String,

    /// Optional display name for the `From` header (e.g. `"Aivyx Agent"`).
    pub from_name: Option<String>,
}

fn default_smtp_port() -> u16 {
    587
}
