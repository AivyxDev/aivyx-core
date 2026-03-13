//! Content redaction filter — prevents credential leakage into public posts.
//!
//! Scans text for patterns that look like API keys, passwords, private keys,
//! bearer tokens, connection strings, and sensitive file paths. If any are
//! detected, the post is **blocked** (not silently redacted) so the agent
//! knows to reformulate.
//!
//! This is defense-in-depth — the publication barrier means agents compose
//! their own content, but LLMs can accidentally include secrets from context.

use regex::Regex;
use std::sync::LazyLock;

/// Result of a redaction check.
#[derive(Debug, Clone)]
pub enum RedactResult {
    /// Content is safe to publish.
    Clean,
    /// Content contains potential credentials — publishing blocked.
    Blocked {
        /// Which pattern categories matched.
        reasons: Vec<String>,
    },
}

impl RedactResult {
    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Blocked { .. })
    }
}

/// A credential pattern to scan for.
struct CredentialPattern {
    name: &'static str,
    regex: Regex,
}

/// The redaction filter — scans content for credential patterns.
pub struct RedactionFilter {
    patterns: Vec<CredentialPattern>,
}

/// Pre-compiled patterns for common credential formats.
static DEFAULT_PATTERNS: LazyLock<Vec<(&str, &str)>> = LazyLock::new(|| {
    vec![
        // API keys — common prefixes
        ("OpenAI API key", r"sk-[a-zA-Z0-9]{20,}"),
        ("Anthropic API key", r#"sk-ant-[a-zA-Z0-9\-]{20,}"#),
        ("Stripe key", r"(?:sk|pk|rk)_(?:live|test)_[a-zA-Z0-9]{10,}"),
        ("AWS access key", r"AKIA[0-9A-Z]{16}"),
        ("GitHub token", r"(?:ghp|gho|ghu|ghs|ghr)_[a-zA-Z0-9]{36,}"),
        ("GitLab token", r#"glpat-[a-zA-Z0-9\-]{20,}"#),
        ("Slack token", r#"xox[bporas]-[a-zA-Z0-9\-]{10,}"#),
        (
            "Discord token",
            r#"[MN][A-Za-z\d]{23,}\.[\w-]{6}\.[\w-]{27,}"#,
        ),
        (
            "Generic API key pattern",
            r#"(?i)api[_-]?key\s*[:=]\s*['"]?[a-zA-Z0-9_\-]{20,}"#,
        ),
        (
            "Generic secret pattern",
            r#"(?i)(?:secret|password|passwd|pwd)\s*[:=]\s*['"]?[^\s'"]{8,}"#,
        ),
        // Private keys
        (
            "PEM private key",
            r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----",
        ),
        (
            "Ed25519 private key bytes",
            r#"(?i)private[_\s]?key\s*[:=]\s*['"]?[A-Za-z0-9+/=]{40,}"#,
        ),
        // Bearer / auth tokens
        ("Bearer token", r#"(?i)bearer\s+[a-zA-Z0-9_\-\.]{20,}"#),
        ("Basic auth", r#"(?i)basic\s+[A-Za-z0-9+/=]{10,}"#),
        // Connection strings
        (
            "Database URL",
            r#"(?i)(?:postgres|mysql|mongodb|redis)://[^\s]{10,}"#,
        ),
        (
            "Connection string",
            r#"(?i)(?:server|host)\s*=\s*[^;\s]+;\s*(?:database|user|password)\s*="#,
        ),
        // JWT tokens (three base64 segments joined by dots)
        (
            "JWT token",
            r#"eyJ[a-zA-Z0-9_-]{10,}\.eyJ[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,}"#,
        ),
        // SSH keys
        (
            "SSH private key path",
            r#"(?i)(?:id_rsa|id_ed25519|id_ecdsa)(?:\s|$|['"])"#,
        ),
        // Hex-encoded secrets (32+ byte keys)
        (
            "Hex secret (64+ chars)",
            r#"(?i)(?:key|secret|token|salt)\s*[:=]\s*['"]?[0-9a-fA-F]{64,}"#,
        ),
    ]
});

impl RedactionFilter {
    /// Create a new filter with the default credential patterns.
    ///
    /// # Panics
    ///
    /// Panics if any built-in regex pattern fails to compile. This is a
    /// programming bug — silently dropping a pattern would let credentials
    /// leak into public posts.
    pub fn new() -> Self {
        let patterns = DEFAULT_PATTERNS
            .iter()
            .map(|(name, pattern)| {
                let regex = Regex::new(pattern).unwrap_or_else(|e| {
                    panic!("redaction pattern '{name}' failed to compile: {e}")
                });
                CredentialPattern { name, regex }
            })
            .collect();

        Self { patterns }
    }

    /// Scan content for credential patterns.
    ///
    /// Returns `RedactResult::Clean` if no patterns match, or
    /// `RedactResult::Blocked` with the matched pattern names.
    pub fn check(&self, content: &str) -> RedactResult {
        let mut reasons = Vec::new();

        for pattern in &self.patterns {
            if pattern.regex.is_match(content) {
                reasons.push(pattern.name.to_string());
            }
        }

        if reasons.is_empty() {
            RedactResult::Clean
        } else {
            tracing::warn!(
                reasons = ?reasons,
                "nexus redaction filter blocked content"
            );
            RedactResult::Blocked { reasons }
        }
    }
}

impl Default for RedactionFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> RedactionFilter {
        RedactionFilter::new()
    }

    #[test]
    fn clean_content_passes() {
        let f = filter();
        let result = f.check("I discovered an interesting pattern in the build logs.");
        assert!(result.is_clean());
    }

    #[test]
    fn normal_code_passes() {
        let f = filter();
        let result = f.check("The function returns a Result<Vec<String>, Error> type.");
        assert!(result.is_clean());
    }

    #[test]
    fn blocks_openai_key() {
        let f = filter();
        let result = f.check("I used sk-proj1234567890abcdefghijk to call the API");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_anthropic_key() {
        let f = filter();
        let result = f.check("The key is sk-ant-api03-abcdefghijklmnopqrstu");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_aws_access_key() {
        let f = filter();
        let result = f.check("AWS credentials: AKIAIOSFODNN7EXAMPLE");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_github_token() {
        let f = filter();
        let result = f.check("Use ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh1234");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_pem_private_key() {
        let f = filter();
        let result = f.check("Here's the key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIE...");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_bearer_token() {
        let f = filter();
        let result = f.check("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_database_url() {
        let f = filter();
        let result = f.check("Connected to postgres://user:password@localhost:5432/mydb");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_jwt_token() {
        let f = filter();
        let result =
            f.check("Token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_generic_password() {
        let f = filter();
        let result = f.check("The password = MyS3cureP@ssw0rd!");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_connection_string() {
        let f = filter();
        let result = f.check("Server=myserver;Database=mydb;User=admin;Password=secret123");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_stripe_key() {
        let f = filter();
        let result = f.check("Stripe key: sk_live_51HGrandomstringhere");
        assert!(result.is_blocked());
    }

    #[test]
    fn blocks_slack_token() {
        let f = filter();
        let result = f.check("Token is xoxb-123456789012-abcdefghij");
        assert!(result.is_blocked());
    }

    #[test]
    fn allows_discussion_about_security() {
        let f = filter();
        let result = f.check(
            "We should rotate API keys regularly and use environment variables for secrets.",
        );
        assert!(result.is_clean());
    }

    #[test]
    fn allows_code_discussion() {
        let f = filter();
        let result = f.check(
            "The `authenticate()` function validates the bearer token format before forwarding.",
        );
        assert!(result.is_clean());
    }

    #[test]
    fn multiple_reasons_reported() {
        let f = filter();
        let result = f.check(
            "I have sk-proj1234567890abcdefghijk and also postgres://admin:pass@db:5432/app",
        );
        match result {
            RedactResult::Blocked { reasons } => {
                assert!(
                    reasons.len() >= 2,
                    "expected multiple reasons, got: {reasons:?}"
                );
            }
            RedactResult::Clean => panic!("expected blocked"),
        }
    }

    #[test]
    fn empty_content_passes() {
        let f = filter();
        assert!(f.check("").is_clean());
    }

    #[test]
    fn unicode_content_passes() {
        let f = filter();
        let result = f.check("エージェントが新しいパターンを発見しました 🎉");
        assert!(result.is_clean());
    }
}
