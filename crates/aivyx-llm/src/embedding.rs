//! Embedding provider abstraction and implementations.
//!
//! Provides the [`EmbeddingProvider`] trait and concrete implementations for
//! Ollama and OpenAI embedding APIs, plus a factory function.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tracing::debug;

use aivyx_config::EmbeddingConfig;
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};

/// A vector embedding produced from a text string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub dimensions: usize,
}

/// Trait for embedding providers.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Provider name (e.g., "ollama", "openai").
    fn name(&self) -> &str;

    /// The dimensionality of embeddings produced by this provider.
    fn dimensions(&self) -> usize;

    /// Embed a single text string.
    async fn embed(&self, text: &str) -> Result<Embedding>;

    /// Embed multiple texts in a batch. Default implementation calls `embed`
    /// sequentially.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Ollama Embedding Provider
// ---------------------------------------------------------------------------

/// Embedding provider using Ollama's native `/api/embed` endpoint.
pub struct OllamaEmbeddingProvider {
    client: Client,
    base_url: String,
    model: String,
    dims: usize,
}

impl OllamaEmbeddingProvider {
    pub fn new(base_url: String, model: String, dims: usize) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(60))
                .connect_timeout(Duration::from_secs(5))
                .pool_max_idle_per_host(4)
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .expect("failed to build HTTP client"),
            base_url,
            model,
            dims,
        }
    }

    fn build_request_body(&self, text: &str) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "input": text,
        })
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<Vec<f32>> {
        let embeddings = body["embeddings"]
            .as_array()
            .ok_or_else(|| AivyxError::Embedding("missing 'embeddings' in response".into()))?;

        let first = embeddings
            .first()
            .ok_or_else(|| AivyxError::Embedding("empty embeddings array".into()))?;

        let vector: Vec<f32> = first
            .as_array()
            .ok_or_else(|| AivyxError::Embedding("embedding is not an array".into()))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(vector)
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, text: &str) -> Result<Embedding> {
        let url = format!("{}/api/embed", self.base_url);
        let body = self.build_request_body(text);

        debug!("Sending Ollama embed request to {url}");

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("Ollama embed request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::Embedding(format!(
                "Ollama embed API error {status}: {error_body}"
            )));
        }

        let response_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AivyxError::Embedding(format!("failed to parse embed response: {e}")))?;

        let vector = self.parse_response(&response_body)?;
        let dimensions = vector.len();

        Ok(Embedding { vector, dimensions })
    }
}

// ---------------------------------------------------------------------------
// OpenAI Embedding Provider
// ---------------------------------------------------------------------------

/// Embedding provider using the OpenAI embeddings API.
pub struct OpenAIEmbeddingProvider {
    client: Client,
    api_key: SecretString,
    model: String,
    dims: usize,
}

impl OpenAIEmbeddingProvider {
    pub fn new(api_key: SecretString, model: String, dims: usize) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10))
                .pool_max_idle_per_host(4)
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .expect("failed to build HTTP client"),
            api_key,
            model,
            dims,
        }
    }

    fn build_request_body(&self, text: &str) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "input": [text],
        })
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<Vec<f32>> {
        let data = body["data"]
            .as_array()
            .ok_or_else(|| AivyxError::Embedding("missing 'data' in response".into()))?;

        let first = data
            .first()
            .ok_or_else(|| AivyxError::Embedding("empty data array".into()))?;

        let vector: Vec<f32> = first["embedding"]
            .as_array()
            .ok_or_else(|| AivyxError::Embedding("embedding is not an array".into()))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(vector)
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIEmbeddingProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, text: &str) -> Result<Embedding> {
        let url = "https://api.openai.com/v1/embeddings";
        let body = self.build_request_body(text);

        debug!("Sending OpenAI embed request");

        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("OpenAI embed request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(AivyxError::RateLimit(format!(
                    "OpenAI embedding rate limited: {error_body}"
                )));
            }
            return Err(AivyxError::Embedding(format!(
                "OpenAI embed API error {status}: {error_body}"
            )));
        }

        let response_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AivyxError::Embedding(format!("failed to parse embed response: {e}")))?;

        let vector = self.parse_response(&response_body)?;
        let dimensions = vector.len();

        Ok(Embedding { vector, dimensions })
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create an embedding provider from configuration, resolving API keys from the encrypted store.
pub fn create_embedding_provider(
    config: &EmbeddingConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> Result<Box<dyn EmbeddingProvider>> {
    match config {
        EmbeddingConfig::Ollama {
            base_url,
            model,
            dimensions,
        } => Ok(Box::new(OllamaEmbeddingProvider::new(
            base_url.clone(),
            model.clone(),
            *dimensions,
        ))),
        EmbeddingConfig::OpenAI {
            api_key_ref,
            model,
            dimensions,
        } => {
            let bytes = store.get(api_key_ref, master_key)?.ok_or_else(|| {
                AivyxError::Config(format!(
                    "API key '{api_key_ref}' not found in encrypted store. Run: aivyx secret set {api_key_ref}"
                ))
            })?;
            let key_str = String::from_utf8(bytes)
                .map_err(|e| AivyxError::Config(format!("API key is not valid UTF-8: {e}")))?;
            Ok(Box::new(OpenAIEmbeddingProvider::new(
                SecretString::from(key_str),
                model.clone(),
                *dimensions,
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_serde_roundtrip() {
        let emb = Embedding {
            vector: vec![0.1, 0.2, 0.3],
            dimensions: 3,
        };
        let json = serde_json::to_string(&emb).unwrap();
        let parsed: Embedding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.vector, emb.vector);
        assert_eq!(parsed.dimensions, 3);
    }

    #[test]
    fn ollama_build_request_body() {
        let provider =
            OllamaEmbeddingProvider::new("http://localhost:11434".into(), "nomic".into(), 768);
        let body = provider.build_request_body("hello world");
        assert_eq!(body["model"], "nomic");
        assert_eq!(body["input"], "hello world");
    }

    #[test]
    fn ollama_parse_response() {
        let provider =
            OllamaEmbeddingProvider::new("http://localhost:11434".into(), "nomic".into(), 768);
        let body = serde_json::json!({
            "embeddings": [[0.1, 0.2, 0.3]]
        });
        let vec = provider.parse_response(&body).unwrap();
        assert_eq!(vec.len(), 3);
        assert!((vec[0] - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn openai_build_request_body() {
        let provider = OpenAIEmbeddingProvider::new(
            SecretString::from("sk-test".to_string()),
            "text-embedding-3-small".into(),
            1536,
        );
        let body = provider.build_request_body("hello world");
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["input"][0], "hello world");
    }

    #[test]
    fn openai_parse_response() {
        let provider = OpenAIEmbeddingProvider::new(
            SecretString::from("sk-test".to_string()),
            "text-embedding-3-small".into(),
            1536,
        );
        let body = serde_json::json!({
            "data": [{"embedding": [0.4, 0.5, 0.6]}]
        });
        let vec = provider.parse_response(&body).unwrap();
        assert_eq!(vec.len(), 3);
        assert!((vec[1] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn dimensions_accessor() {
        let ollama =
            OllamaEmbeddingProvider::new("http://localhost:11434".into(), "nomic".into(), 768);
        assert_eq!(ollama.dimensions(), 768);

        let openai = OpenAIEmbeddingProvider::new(
            SecretString::from("sk-test".to_string()),
            "text-embedding-3-small".into(),
            1536,
        );
        assert_eq!(openai.dimensions(), 1536);
    }

    #[test]
    fn factory_selects_ollama() {
        let config = EmbeddingConfig::default();
        // Factory with Ollama doesn't need the store or master_key
        // but we can't easily create them here, so just verify the config is Ollama
        match config {
            EmbeddingConfig::Ollama { .. } => {}
            _ => panic!("expected Ollama default"),
        }
    }
}
