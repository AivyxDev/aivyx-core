//! Unified error type for the aivyx framework.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AivyxError {
    /// Filesystem or other I/O failure.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// TOML serialization failure.
    #[error("TOML serialization error: {0}")]
    TomlSer(String),

    /// TOML deserialization failure.
    #[error("TOML deserialization error: {0}")]
    TomlDe(String),

    /// Cryptographic operation failure (encryption, decryption, KDF).
    #[error("crypto error: {0}")]
    Crypto(String),

    /// A capability check rejected the requested action.
    #[error("capability denied: {0}")]
    CapabilityDenied(String),

    /// The referenced capability does not exist.
    #[error("capability not found: {0}")]
    CapabilityNotFound(String),

    /// The audit log's HMAC chain is broken or an entry is malformed.
    #[error("audit integrity violation: {0}")]
    AuditIntegrity(String),

    /// Configuration loading, parsing, or validation failure.
    #[error("config error: {0}")]
    Config(String),

    /// Encrypted store (redb) operation failure.
    #[error("storage error: {0}")]
    Storage(String),

    /// The aivyx data directory has not been initialized yet.
    #[error("not initialized: {0}")]
    NotInitialized(String),

    /// LLM provider error (API call failure, bad response, etc.).
    #[error("LLM provider error: {0}")]
    LlmProvider(String),

    /// HTTP request/response error.
    #[error("HTTP error: {0}")]
    Http(String),

    /// Rate limit exceeded.
    #[error("rate limit exceeded: {0}")]
    RateLimit(String),

    /// Agent runtime error.
    #[error("agent error: {0}")]
    Agent(String),

    /// Embedding provider error (API call failure, bad response, etc.).
    #[error("embedding error: {0}")]
    Embedding(String),

    /// Memory system error (storage, retrieval, index).
    #[error("memory error: {0}")]
    Memory(String),

    /// Task orchestration error (planning, execution, checkpoint).
    #[error("task error: {0}")]
    Task(String),

    /// Scheduler error (cron parsing, store, or runtime failure).
    #[error("scheduler error: {0}")]
    Scheduler(String),

    /// Inbound channel error (connection, auth, message handling).
    #[error("channel error: {0}")]
    Channel(String),

    /// Wraps another error with additional context.
    #[error("{message}")]
    Context {
        /// Human-readable context describing what was happening when the error occurred.
        message: String,
        /// The underlying error.
        #[source]
        source: Box<AivyxError>,
    },

    /// Catch-all for errors that don't fit another variant.
    #[error("{0}")]
    Other(String),
}

impl AivyxError {
    /// Whether this error is transient and the operation should be retried.
    ///
    /// Delegates through `Context` wrappers to check the underlying error.
    pub fn is_retryable(&self) -> bool {
        match self {
            AivyxError::RateLimit(_) | AivyxError::Http(_) => true,
            AivyxError::Context { source, .. } => source.is_retryable(),
            _ => false,
        }
    }
}

/// Extension trait for adding context to `Result<T, AivyxError>`.
///
/// ```
/// use aivyx_core::{AivyxError, Result, ResultExt};
///
/// fn load_config() -> Result<String> {
///     Err(AivyxError::Io(std::io::Error::new(
///         std::io::ErrorKind::NotFound, "missing",
///     )))
///     .context("loading main config")
/// }
/// ```
pub trait ResultExt<T> {
    /// Wrap the error with a static context message.
    fn context(self, msg: impl Into<String>) -> Result<T>;

    /// Wrap the error with a lazily-computed context message.
    fn with_context(self, f: impl FnOnce() -> String) -> Result<T>;
}

impl<T> ResultExt<T> for Result<T> {
    fn context(self, msg: impl Into<String>) -> Result<T> {
        self.map_err(|e| AivyxError::Context {
            message: msg.into(),
            source: Box::new(e),
        })
    }

    fn with_context(self, f: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| AivyxError::Context {
            message: f(),
            source: Box::new(e),
        })
    }
}

/// Convenience alias for `Result<T, AivyxError>`.
pub type Result<T> = std::result::Result<T, AivyxError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_wraps_error() {
        let inner = AivyxError::Config("bad value".into());
        let result: Result<()> = Err(inner);
        let wrapped = result.context("loading provider config");

        let err = wrapped.unwrap_err();
        assert_eq!(err.to_string(), "loading provider config");
        assert!(matches!(
            err,
            AivyxError::Context {
                ref source, ..
            } if matches!(**source, AivyxError::Config(_))
        ));
    }

    #[test]
    fn with_context_lazy() {
        let path = "/etc/aivyx.toml";
        let result: Result<()> = Err(AivyxError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing",
        )));
        let wrapped = result.with_context(|| format!("reading {path}"));

        let err = wrapped.unwrap_err();
        assert_eq!(err.to_string(), "reading /etc/aivyx.toml");
    }

    #[test]
    fn retryable_through_context() {
        let rate_limit = AivyxError::RateLimit("429".into());
        assert!(rate_limit.is_retryable());

        // Wrapping in Context should still be retryable
        let wrapped = AivyxError::Context {
            message: "calling provider".into(),
            source: Box::new(AivyxError::RateLimit("429".into())),
        };
        assert!(wrapped.is_retryable());

        // Non-retryable wrapped should stay non-retryable
        let non_retryable = AivyxError::Context {
            message: "doing stuff".into(),
            source: Box::new(AivyxError::Config("bad".into())),
        };
        assert!(!non_retryable.is_retryable());
    }
}
