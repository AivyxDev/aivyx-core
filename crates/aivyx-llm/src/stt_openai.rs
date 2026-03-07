//! OpenAI Whisper speech-to-text provider.
//!
//! Calls `POST https://api.openai.com/v1/audio/transcriptions` with
//! multipart form data containing the audio file.

use async_trait::async_trait;
use serde::Deserialize;

use aivyx_core::{AivyxError, Result};

use crate::stt::{AudioFormat, SttProvider, TranscriptionResult};

/// OpenAI Whisper STT provider.
pub struct OpenAiSttProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiSttProvider {
    /// Create a new OpenAI Whisper STT provider.
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }
}

/// OpenAI Whisper API verbose response format.
#[derive(Debug, Deserialize)]
struct WhisperResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}

#[async_trait]
impl SttProvider for OpenAiSttProvider {
    fn name(&self) -> &str {
        "openai-whisper"
    }

    async fn transcribe(
        &self,
        audio: &[u8],
        format: AudioFormat,
    ) -> Result<TranscriptionResult> {
        let extension = match format {
            AudioFormat::Wav => "wav",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Flac => "flac",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Webm => "webm",
            AudioFormat::M4a => "m4a",
            AudioFormat::Unknown => "wav",
        };

        let file_part = reqwest::multipart::Part::bytes(audio.to_vec())
            .file_name(format!("audio.{extension}"))
            .mime_str(format.mime_type())
            .map_err(|e| AivyxError::Http(e.to_string()))?;

        let form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "verbose_json")
            .part("file", file_part);

        let resp = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| AivyxError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "unknown error".into());
            return Err(AivyxError::LlmProvider(format!(
                "OpenAI Whisper API error ({status}): {body}"
            )));
        }

        let whisper: WhisperResponse = resp
            .json()
            .await
            .map_err(|e| AivyxError::Http(format!("failed to parse Whisper response: {e}")))?;

        Ok(TranscriptionResult {
            text: whisper.text,
            language: whisper.language,
            duration_secs: whisper.duration,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name() {
        let provider = OpenAiSttProvider::new("test-key".into(), "whisper-1".into());
        assert_eq!(provider.name(), "openai-whisper");
    }
}
