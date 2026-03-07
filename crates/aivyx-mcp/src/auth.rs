//! MCP OAuth 2.1 client with PKCE support.
//!
//! Implements the OAuth 2.1 authorization code flow with PKCE for authenticating
//! with remote MCP servers that require OAuth. Supports:
//! - Server metadata discovery via `/.well-known/oauth-authorization-server`
//! - PKCE code verifier/challenge generation (S256 method)
//! - Token exchange and refresh
//!
//! Tokens can be stored encrypted via `aivyx-crypto`'s `EncryptedStore`.

use aivyx_core::{AivyxError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// OAuth 2.1 client for MCP server authentication.
pub struct McpOAuthClient {
    /// OAuth client ID registered with the MCP server.
    client_id: String,
    /// Requested scopes.
    scopes: Vec<String>,
    /// Token endpoint URL (discovered or configured).
    token_endpoint: String,
    /// Authorization endpoint URL (discovered or configured).
    authorization_endpoint: String,
    /// HTTP client for token requests.
    http: reqwest::Client,
}

/// OAuth server metadata (RFC 8414 — discovered from well-known endpoint).
#[derive(Debug, Clone, Deserialize)]
pub struct OAuthMetadata {
    /// URL of the authorization endpoint.
    pub authorization_endpoint: String,
    /// URL of the token endpoint.
    pub token_endpoint: String,
    /// Supported response types.
    #[serde(default)]
    pub response_types_supported: Vec<String>,
    /// Supported grant types.
    #[serde(default)]
    pub grant_types_supported: Vec<String>,
    /// Supported PKCE code challenge methods.
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
}

/// OAuth token response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// The access token for API requests.
    pub access_token: String,
    /// Refresh token for obtaining new access tokens (if issued).
    pub refresh_token: Option<String>,
    /// When the access token expires (seconds from issuance).
    pub expires_in: Option<u64>,
    /// Token type (typically "Bearer").
    pub token_type: String,
}

/// PKCE verifier/challenge pair for the authorization code flow.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    /// The code verifier (random string sent during token exchange).
    pub verifier: String,
    /// The code challenge (S256 hash of verifier, sent during authorization).
    pub challenge: String,
}

impl McpOAuthClient {
    /// Discover OAuth metadata from the server's well-known endpoint and
    /// create a configured client.
    pub async fn discover(server_url: &str, client_id: &str, scopes: Vec<String>) -> Result<Self> {
        let base = server_url.trim_end_matches('/');
        let metadata_url = format!("{base}/.well-known/oauth-authorization-server");

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| AivyxError::Other(format!("HTTP client error: {e}")))?;

        let resp = http
            .get(&metadata_url)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("OAuth discovery failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AivyxError::Http(format!(
                "OAuth discovery returned HTTP {}",
                resp.status()
            )));
        }

        let metadata: OAuthMetadata = resp
            .json()
            .await
            .map_err(|e| AivyxError::Other(format!("OAuth metadata parse error: {e}")))?;

        // Verify S256 PKCE is supported
        if !metadata.code_challenge_methods_supported.is_empty()
            && !metadata
                .code_challenge_methods_supported
                .contains(&"S256".to_string())
        {
            return Err(AivyxError::Other(
                "MCP OAuth server does not support S256 PKCE code challenge method".into(),
            ));
        }

        Ok(Self {
            client_id: client_id.to_string(),
            scopes,
            token_endpoint: metadata.token_endpoint,
            authorization_endpoint: metadata.authorization_endpoint,
            http,
        })
    }

    /// Create a client with explicit endpoint URLs (no discovery).
    pub fn new(
        client_id: &str,
        scopes: Vec<String>,
        authorization_endpoint: &str,
        token_endpoint: &str,
    ) -> Self {
        Self {
            client_id: client_id.to_string(),
            scopes,
            token_endpoint: token_endpoint.to_string(),
            authorization_endpoint: authorization_endpoint.to_string(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Generate a PKCE verifier/challenge pair.
    ///
    /// Uses a cryptographically random 32-byte verifier, base64url-encoded,
    /// with an S256 code challenge (SHA-256 hash of the verifier).
    pub fn generate_pkce() -> PkceChallenge {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let verifier = base64url_encode(&bytes);
        let challenge = s256_challenge(&verifier);
        PkceChallenge {
            verifier,
            challenge,
        }
    }

    /// Build the authorization URL for the user to visit.
    ///
    /// Returns the URL and the PKCE challenge (caller must store the verifier
    /// for use in `exchange_code`).
    pub fn authorization_url(&self, redirect_uri: &str) -> (String, PkceChallenge) {
        let pkce = Self::generate_pkce();
        let scope = self.scopes.join(" ");

        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256",
            self.authorization_endpoint,
            urlencoding::encode(&self.client_id),
            urlencoding::encode(redirect_uri),
            urlencoding::encode(&scope),
            urlencoding::encode(&pkce.challenge),
        );

        (url, pkce)
    }

    /// Exchange an authorization code for tokens using the PKCE verifier.
    pub async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<OAuthTokens> {
        let resp = self
            .http
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", &self.client_id),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("OAuth token exchange failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AivyxError::Http(format!(
                "OAuth token exchange returned HTTP {status}: {body}"
            )));
        }

        resp.json::<OAuthTokens>()
            .await
            .map_err(|e| AivyxError::Other(format!("OAuth token parse error: {e}")))
    }

    /// Refresh an expired access token using a refresh token.
    pub async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokens> {
        let resp = self
            .http
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.client_id),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("OAuth token refresh failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AivyxError::Http(format!(
                "OAuth token refresh returned HTTP {status}: {body}"
            )));
        }

        resp.json::<OAuthTokens>()
            .await
            .map_err(|e| AivyxError::Other(format!("OAuth token refresh parse error: {e}")))
    }

    /// Get the authorization endpoint URL.
    pub fn authorization_endpoint(&self) -> &str {
        &self.authorization_endpoint
    }

    /// Get the token endpoint URL.
    pub fn token_endpoint(&self) -> &str {
        &self.token_endpoint
    }
}

/// Base64url-encode bytes without padding (per RFC 7636).
fn base64url_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute the S256 code challenge: base64url(sha256(verifier)).
fn s256_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    base64url_encode(&hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_deterministic_for_same_verifier() {
        let challenge1 = s256_challenge("test-verifier");
        let challenge2 = s256_challenge("test-verifier");
        assert_eq!(challenge1, challenge2);
    }

    #[test]
    fn pkce_challenge_differs_for_different_verifiers() {
        let c1 = s256_challenge("verifier-a");
        let c2 = s256_challenge("verifier-b");
        assert_ne!(c1, c2);
    }

    #[test]
    fn generate_pkce_creates_valid_pair() {
        let pkce = McpOAuthClient::generate_pkce();
        assert!(!pkce.verifier.is_empty());
        assert!(!pkce.challenge.is_empty());
        // Verify the challenge matches the verifier
        let expected = s256_challenge(&pkce.verifier);
        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn base64url_encode_no_padding() {
        let encoded = base64url_encode(&[0, 1, 2, 3]);
        assert!(!encoded.contains('='));
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn oauth_tokens_deserializes() {
        let json = r#"{
            "access_token": "eyJ...",
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "dGVzdA=="
        }"#;
        let tokens: OAuthTokens = serde_json::from_str(json).unwrap();
        assert_eq!(tokens.token_type, "Bearer");
        assert_eq!(tokens.expires_in, Some(3600));
        assert!(tokens.refresh_token.is_some());
    }

    #[test]
    fn oauth_tokens_without_refresh() {
        let json = r#"{
            "access_token": "eyJ...",
            "token_type": "Bearer"
        }"#;
        let tokens: OAuthTokens = serde_json::from_str(json).unwrap();
        assert!(tokens.refresh_token.is_none());
        assert!(tokens.expires_in.is_none());
    }

    #[test]
    fn oauth_metadata_deserializes() {
        let json = r#"{
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token",
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"]
        }"#;
        let metadata: OAuthMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(
            metadata.authorization_endpoint,
            "https://auth.example.com/authorize"
        );
        assert!(metadata
            .code_challenge_methods_supported
            .contains(&"S256".to_string()));
    }

    #[test]
    fn new_client_stores_endpoints() {
        let client = McpOAuthClient::new(
            "test-client",
            vec!["read".into()],
            "https://auth.example.com/authorize",
            "https://auth.example.com/token",
        );
        assert_eq!(
            client.authorization_endpoint(),
            "https://auth.example.com/authorize"
        );
        assert_eq!(
            client.token_endpoint(),
            "https://auth.example.com/token"
        );
    }
}
