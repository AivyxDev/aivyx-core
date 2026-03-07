//! Ollama speech-to-text provider.
//!
//! Uses Ollama's generate endpoint with audio data encoded as base64.
//! Ollama's Whisper support is experimental — the audio is sent via
//! the `images` field (which accepts any base64 binary data).

use async_trait::async_trait;

use aivyx_core::{AivyxError, Result};

use crate::stt::{AudioFormat, SttProvider, TranscriptionResult};

/// Ollama STT provider.
pub struct OllamaSttProvider {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OllamaSttProvider {
    /// Create a new Ollama STT provider.
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            base_url,
            model,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SttProvider for OllamaSttProvider {
    fn name(&self) -> &str {
        "ollama-stt"
    }

    async fn transcribe(&self, audio: &[u8], _format: AudioFormat) -> Result<TranscriptionResult> {
        use base64::Engine;
        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(audio);

        let resp = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&serde_json::json!({
                "model": self.model,
                "prompt": "Transcribe the following audio to text.",
                "images": [audio_b64],
                "stream": false,
            }))
            .send()
            .await
            .map_err(|e| AivyxError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "unknown error".into());
            return Err(AivyxError::LlmProvider(format!(
                "Ollama transcription error ({status}): {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AivyxError::Http(format!("failed to parse Ollama response: {e}")))?;

        let text = body["response"].as_str().unwrap_or("").to_string();

        Ok(TranscriptionResult {
            text,
            language: None,
            duration_secs: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name() {
        let provider = OllamaSttProvider::new("http://localhost:11434".into(), "whisper".into());
        assert_eq!(provider.name(), "ollama-stt");
    }
}
