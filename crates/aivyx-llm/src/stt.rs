//! Speech-to-text provider abstraction.
//!
//! Defines [`SttProvider`] trait for transcribing audio to text.
//! Implementations live in separate modules (`stt_openai`, `stt_ollama`).

use async_trait::async_trait;

use aivyx_core::Result;

/// Result of a speech-to-text transcription.
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// The transcribed text.
    pub text: String,
    /// Detected language (ISO 639-1 code), if reported by the provider.
    pub language: Option<String>,
    /// Duration of the audio in seconds, if reported by the provider.
    pub duration_secs: Option<f64>,
}

/// Audio format hint for transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
    Ogg,
    Webm,
    M4a,
    Unknown,
}

impl AudioFormat {
    /// Infer format from a filename extension.
    pub fn from_filename(filename: &str) -> Self {
        let ext = filename.rsplit('.').next().unwrap_or("");
        match ext.to_lowercase().as_str() {
            "wav" => Self::Wav,
            "mp3" => Self::Mp3,
            "flac" => Self::Flac,
            "ogg" => Self::Ogg,
            "webm" => Self::Webm,
            "m4a" => Self::M4a,
            _ => Self::Unknown,
        }
    }

    /// MIME type string for the format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Flac => "audio/flac",
            Self::Ogg => "audio/ogg",
            Self::Webm => "audio/webm",
            Self::M4a => "audio/mp4",
            Self::Unknown => "application/octet-stream",
        }
    }
}

/// Trait for speech-to-text providers.
///
/// Implementations receive raw audio bytes and return transcribed text.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Provider name (e.g., "openai-whisper", "ollama").
    fn name(&self) -> &str;

    /// Transcribe audio bytes to text.
    ///
    /// * `audio` — Raw audio data in the specified format.
    /// * `format` — Audio format hint (used for MIME type in API calls).
    async fn transcribe(&self, audio: &[u8], format: AudioFormat) -> Result<TranscriptionResult>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_format_from_filename() {
        assert_eq!(
            AudioFormat::from_filename("recording.mp3"),
            AudioFormat::Mp3
        );
        assert_eq!(AudioFormat::from_filename("audio.wav"), AudioFormat::Wav);
        assert_eq!(AudioFormat::from_filename("voice.flac"), AudioFormat::Flac);
        assert_eq!(AudioFormat::from_filename("speech.ogg"), AudioFormat::Ogg);
        assert_eq!(
            AudioFormat::from_filename("meeting.webm"),
            AudioFormat::Webm
        );
        assert_eq!(AudioFormat::from_filename("call.m4a"), AudioFormat::M4a);
        assert_eq!(
            AudioFormat::from_filename("unknown.xyz"),
            AudioFormat::Unknown
        );
    }

    #[test]
    fn audio_format_mime_types() {
        assert_eq!(AudioFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(AudioFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AudioFormat::Unknown.mime_type(), "application/octet-stream");
    }
}
