//! OpenAI text-to-speech provider.
//!
//! Calls `POST https://api.openai.com/v1/audio/speech` with the
//! text input and voice configuration. Returns raw audio bytes.

use async_trait::async_trait;

use aivyx_core::{AivyxError, Result};

use crate::tts::{TtsAudioFormat, TtsOptions, TtsOutput, TtsProvider};

/// OpenAI TTS provider (tts-1, tts-1-hd).
pub struct OpenAiTtsProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiTtsProvider {
    /// Create a new OpenAI TTS provider.
    ///
    /// * `api_key` — OpenAI API key.
    /// * `model` — Model name (e.g., "tts-1", "tts-1-hd").
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }

    /// Map our format enum to OpenAI's response_format parameter.
    fn openai_format(format: TtsAudioFormat) -> &'static str {
        match format {
            TtsAudioFormat::Mp3 => "mp3",
            TtsAudioFormat::Opus => "opus",
            TtsAudioFormat::Pcm16Le => "pcm",
        }
    }
}

#[async_trait]
impl TtsProvider for OpenAiTtsProvider {
    fn name(&self) -> &str {
        "openai-tts"
    }

    async fn synthesize(
        &self,
        text: &str,
        options: &TtsOptions,
    ) -> Result<TtsOutput> {
        let response_format = Self::openai_format(options.format);

        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": options.voice,
            "speed": options.speed,
            "response_format": response_format,
        });

        let resp = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "unknown error".into());
            return Err(AivyxError::LlmProvider(format!(
                "OpenAI TTS API error ({status}): {body}"
            )));
        }

        let audio = resp
            .bytes()
            .await
            .map_err(|e| AivyxError::Http(format!("failed to read TTS audio response: {e}")))?
            .to_vec();

        // OpenAI returns 24kHz for PCM, 24kHz for opus, variable for mp3
        let sample_rate = match options.format {
            TtsAudioFormat::Pcm16Le => 24_000,
            TtsAudioFormat::Opus => 24_000,
            TtsAudioFormat::Mp3 => 24_000,
        };

        Ok(TtsOutput {
            audio,
            format: options.format,
            sample_rate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name() {
        let provider = OpenAiTtsProvider::new("test-key".into(), "tts-1".into());
        assert_eq!(provider.name(), "openai-tts");
    }

    #[test]
    fn format_mapping() {
        assert_eq!(OpenAiTtsProvider::openai_format(TtsAudioFormat::Mp3), "mp3");
        assert_eq!(
            OpenAiTtsProvider::openai_format(TtsAudioFormat::Opus),
            "opus"
        );
        assert_eq!(
            OpenAiTtsProvider::openai_format(TtsAudioFormat::Pcm16Le),
            "pcm"
        );
    }
}
