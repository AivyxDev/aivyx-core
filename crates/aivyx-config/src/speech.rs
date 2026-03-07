//! Speech configuration for voice input (STT) and voice output (TTS).
//!
//! [`SpeechConfig`] defines which providers to use for speech-to-text
//! (OpenAI Whisper API or local Ollama) and text-to-speech (OpenAI TTS
//! or edge-tts).

use serde::{Deserialize, Serialize};

/// Configuration for voice features (STT + TTS).
///
/// The existing `provider` and `model` fields configure STT (backward
/// compatible). The optional `tts` field enables text-to-speech.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechConfig {
    /// Which speech-to-text provider to use.
    pub provider: SpeechProvider,
    /// Model name for transcription (e.g., "whisper-1").
    #[serde(default = "default_speech_model")]
    pub model: String,
    /// Optional text-to-speech configuration.
    #[serde(default)]
    pub tts: Option<TtsConfig>,
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

/// Text-to-speech configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Which TTS provider to use.
    pub provider: TtsProvider,
    /// Model name for synthesis (e.g., "tts-1", "tts-1-hd").
    #[serde(default = "default_tts_model")]
    pub model: String,
    /// Default voice name.
    #[serde(default = "default_tts_voice")]
    pub voice: String,
    /// Default speech speed (1.0 = normal).
    #[serde(default = "default_tts_speed")]
    pub speed: f32,
}

/// Text-to-speech provider backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TtsProvider {
    /// OpenAI TTS API (cloud).
    #[serde(rename = "openai")]
    OpenAi {
        /// Reference to the API key in the encrypted store.
        api_key_ref: String,
    },
    /// Edge TTS (free, via Microsoft Edge's synthesis service).
    /// Requires `edge-tts` CLI: `pip install edge-tts`.
    #[serde(rename = "edge")]
    Edge,
}

fn default_speech_model() -> String {
    "whisper-1".into()
}

fn default_tts_model() -> String {
    "tts-1".into()
}

fn default_tts_voice() -> String {
    "alloy".into()
}

fn default_tts_speed() -> f32 {
    1.0
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
            tts: None,
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
            tts: None,
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

    #[test]
    fn speech_config_backward_compat_no_tts() {
        // Existing configs without tts field should still work
        let toml_str = r#"
            model = "whisper-1"
            [provider]
            type = "openai"
            api_key_ref = "openai-key"
        "#;
        let config: SpeechConfig = toml::from_str(toml_str).unwrap();
        assert!(config.tts.is_none());
    }

    #[test]
    fn speech_config_with_openai_tts() {
        let toml_str = r#"
            model = "whisper-1"
            [provider]
            type = "openai"
            api_key_ref = "openai-key"
            [tts]
            model = "tts-1-hd"
            voice = "nova"
            speed = 1.2
            [tts.provider]
            type = "openai"
            api_key_ref = "openai-key"
        "#;
        let config: SpeechConfig = toml::from_str(toml_str).unwrap();
        let tts = config.tts.unwrap();
        assert_eq!(tts.model, "tts-1-hd");
        assert_eq!(tts.voice, "nova");
        assert!((tts.speed - 1.2).abs() < f32::EPSILON);
        assert!(matches!(tts.provider, TtsProvider::OpenAi { .. }));
    }

    #[test]
    fn speech_config_with_edge_tts() {
        let toml_str = r#"
            model = "whisper-1"
            [provider]
            type = "ollama"
            [tts]
            voice = "en-US-GuyNeural"
            [tts.provider]
            type = "edge"
        "#;
        let config: SpeechConfig = toml::from_str(toml_str).unwrap();
        let tts = config.tts.unwrap();
        assert_eq!(tts.voice, "en-US-GuyNeural");
        assert_eq!(tts.model, "tts-1"); // default
        assert!(matches!(tts.provider, TtsProvider::Edge));
    }

    #[test]
    fn tts_config_defaults() {
        let json = r#"{"provider":{"type":"edge"}}"#;
        let config: TtsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model, "tts-1");
        assert_eq!(config.voice, "alloy");
        assert!((config.speed - 1.0).abs() < f32::EPSILON);
    }
}
