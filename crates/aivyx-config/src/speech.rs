//! Speech-to-text configuration for voice input.
//!
//! [`SpeechConfig`] defines which transcription provider to use (OpenAI
//! Whisper API or local Ollama) and the model name.

use serde::{Deserialize, Serialize};

/// Configuration for speech-to-text transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechConfig {
    /// Which speech-to-text provider to use.
    pub provider: SpeechProvider,
    /// Model name for transcription (e.g., "whisper-1").
    #[serde(default = "default_speech_model")]
    pub model: String,
}

/// Speech-to-text provider backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SpeechProvider {
    /// OpenAI Whisper API (cloud).
    #[serde(rename = "openai")]
    OpenAi {
        /// Reference to the API key in the encrypted store.
        api_key_ref: String,
    },
    /// Ollama local speech model.
    #[serde(rename = "ollama")]
    Ollama {
        /// Base URL for the Ollama server. Defaults to `http://localhost:11434`.
        #[serde(default)]
        base_url: Option<String>,
    },
}

fn default_speech_model() -> String {
    "whisper-1".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_config_openai_serde() {
        let config = SpeechConfig {
            provider: SpeechProvider::OpenAi {
                api_key_ref: "openai-key".into(),
            },
            model: "whisper-1".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type\":\"openai\""));
        assert!(json.contains("openai-key"));

        let restored: SpeechConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.model, "whisper-1");
        if let SpeechProvider::OpenAi { api_key_ref } = restored.provider {
            assert_eq!(api_key_ref, "openai-key");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn speech_config_ollama_serde() {
        let config = SpeechConfig {
            provider: SpeechProvider::Ollama {
                base_url: Some("http://localhost:11434".into()),
            },
            model: "whisper".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type\":\"ollama\""));

        let restored: SpeechConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.model, "whisper");
    }

    #[test]
    fn speech_config_default_model() {
        let json = r#"{"provider":{"type":"ollama"},"model":"whisper-1"}"#;
        let config: SpeechConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model, "whisper-1");

        // Test serde default
        let toml_str = r#"
            model = "whisper-1"
            [provider]
            type = "ollama"
        "#;
        let config: SpeechConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model, "whisper-1");
    }
}
