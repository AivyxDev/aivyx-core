//! Embedding provider configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the embedding provider used by the memory system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EmbeddingConfig {
    /// Ollama local embedding provider (no API key needed).
    Ollama {
        base_url: String,
        model: String,
        dimensions: usize,
    },
    /// OpenAI embedding provider (requires API key).
    OpenAI {
        api_key_ref: String,
        model: String,
        dimensions: usize,
    },
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self::Ollama {
            base_url: "http://localhost:11434".into(),
            model: "nomic-embed-text".into(),
            dimensions: 768,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_ollama() {
        let config = EmbeddingConfig::default();
        match config {
            EmbeddingConfig::Ollama {
                base_url,
                model,
                dimensions,
            } => {
                assert_eq!(base_url, "http://localhost:11434");
                assert_eq!(model, "nomic-embed-text");
                assert_eq!(dimensions, 768);
            }
            _ => panic!("expected Ollama"),
        }
    }

    #[test]
    fn toml_roundtrip_ollama() {
        let config = EmbeddingConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: EmbeddingConfig = toml::from_str(&toml_str).unwrap();
        match parsed {
            EmbeddingConfig::Ollama { model, .. } => assert_eq!(model, "nomic-embed-text"),
            _ => panic!("expected Ollama"),
        }
    }

    #[test]
    fn toml_roundtrip_openai() {
        let config = EmbeddingConfig::OpenAI {
            api_key_ref: "openai-key".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
        };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: EmbeddingConfig = toml::from_str(&toml_str).unwrap();
        match parsed {
            EmbeddingConfig::OpenAI {
                api_key_ref,
                model,
                dimensions,
            } => {
                assert_eq!(api_key_ref, "openai-key");
                assert_eq!(model, "text-embedding-3-small");
                assert_eq!(dimensions, 1536);
            }
            _ => panic!("expected OpenAI"),
        }
    }
}
