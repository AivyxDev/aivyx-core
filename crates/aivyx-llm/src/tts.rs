//! Text-to-speech provider abstraction.
//!
//! Defines [`TtsProvider`] trait for synthesizing speech from text.
//! Implementations live in separate modules (`tts_openai`, `tts_edge`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use aivyx_core::Result;

/// Audio output format for TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TtsAudioFormat {
    /// Raw 16-bit little-endian PCM.
    Pcm16Le,
    /// Opus encoded audio.
    Opus,
    /// MP3 encoded audio.
    Mp3,
}

impl TtsAudioFormat {
    /// MIME type string for the format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Pcm16Le => "audio/pcm",
            Self::Opus => "audio/opus",
            Self::Mp3 => "audio/mpeg",
        }
    }
}

/// Options for text-to-speech synthesis.
#[derive(Debug, Clone)]
pub struct TtsOptions {
    /// Voice name/ID (provider-specific, e.g., "alloy", "nova", "en-US-GuyNeural").
    pub voice: String,
    /// Speech speed multiplier (1.0 = normal).
    pub speed: f32,
    /// Desired output format.
    pub format: TtsAudioFormat,
}

impl Default for TtsOptions {
    fn default() -> Self {
        Self {
            voice: "alloy".into(),
            speed: 1.0,
            format: TtsAudioFormat::Mp3,
        }
    }
}

/// Result of a TTS synthesis operation.
pub struct TtsOutput {
    /// Raw audio bytes in the requested format.
    pub audio: Vec<u8>,
    /// Actual output format (may differ from requested if provider doesn't support it).
    pub format: TtsAudioFormat,
    /// Sample rate in Hz (relevant for PCM output).
    pub sample_rate: u32,
}

/// Trait for text-to-speech providers.
///
/// Implementations synthesize speech audio from text input.
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Provider name (e.g., "openai-tts", "edge-tts").
    fn name(&self) -> &str;

    /// Synthesize speech from text.
    async fn synthesize(&self, text: &str, options: &TtsOptions) -> Result<TtsOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tts_options() {
        let opts = TtsOptions::default();
        assert_eq!(opts.voice, "alloy");
        assert!((opts.speed - 1.0).abs() < f32::EPSILON);
        assert_eq!(opts.format, TtsAudioFormat::Mp3);
    }

    #[test]
    fn tts_audio_format_mime_types() {
        assert_eq!(TtsAudioFormat::Pcm16Le.mime_type(), "audio/pcm");
        assert_eq!(TtsAudioFormat::Opus.mime_type(), "audio/opus");
        assert_eq!(TtsAudioFormat::Mp3.mime_type(), "audio/mpeg");
    }

    #[test]
    fn tts_audio_format_serde() {
        let json = serde_json::to_string(&TtsAudioFormat::Pcm16Le).unwrap();
        assert_eq!(json, "\"pcm16_le\"");
        let restored: TtsAudioFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, TtsAudioFormat::Pcm16Le);
    }
}
