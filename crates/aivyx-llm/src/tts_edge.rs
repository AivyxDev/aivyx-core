//! Edge TTS provider — free text-to-speech via Microsoft Edge's synthesis.
//!
//! Uses the `edge-tts` command-line tool (`pip install edge-tts`) to
//! synthesize speech. This avoids depending on a third-party Rust crate
//! for the Edge TTS WebSocket protocol while still providing free,
//! high-quality neural TTS voices.
//!
//! # Available voices
//!
//! Run `edge-tts --list-voices` to see available voices. Examples:
//! - `en-US-GuyNeural` (male, US English)
//! - `en-US-JennyNeural` (female, US English)
//! - `en-GB-SoniaNeural` (female, British English)

use async_trait::async_trait;
use tokio::process::Command;

use aivyx_core::{AivyxError, Result};

use crate::tts::{TtsAudioFormat, TtsOptions, TtsOutput, TtsProvider};

/// Edge TTS provider using the `edge-tts` CLI tool.
///
/// Requires `edge-tts` to be installed: `pip install edge-tts`.
pub struct EdgeTtsProvider;

impl EdgeTtsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgeTtsProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TtsProvider for EdgeTtsProvider {
    fn name(&self) -> &str {
        "edge-tts"
    }

    async fn synthesize(
        &self,
        text: &str,
        options: &TtsOptions,
    ) -> Result<TtsOutput> {
        // edge-tts outputs MP3 by default. We always request MP3 and note
        // the format in the output.
        let output_format = TtsAudioFormat::Mp3;

        // Create a temp file for the output
        let temp_dir = std::env::temp_dir();
        let output_path = temp_dir.join(format!(
            "aivyx-tts-{}.mp3",
            uuid::Uuid::new_v4()
        ));

        let mut cmd = Command::new("edge-tts");
        cmd.arg("--voice")
            .arg(&options.voice)
            .arg("--text")
            .arg(text)
            .arg("--write-media")
            .arg(&output_path);

        // Apply speech rate if not default
        if (options.speed - 1.0).abs() > 0.01 {
            // edge-tts uses percentage format: +50% for 1.5x, -25% for 0.75x
            let rate_pct = ((options.speed - 1.0) * 100.0) as i32;
            let rate_str = if rate_pct >= 0 {
                format!("+{rate_pct}%")
            } else {
                format!("{rate_pct}%")
            };
            cmd.arg("--rate").arg(rate_str);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    AivyxError::Config(
                        "edge-tts not found. Install with: pip install edge-tts".into(),
                    )
                } else {
                    AivyxError::Other(format!("edge-tts execution failed: {e}"))
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up temp file on error
            let _ = tokio::fs::remove_file(&output_path).await;
            return Err(AivyxError::Other(format!(
                "edge-tts failed (exit {}): {stderr}",
                output.status
            )));
        }

        // Read the output audio file
        let audio = tokio::fs::read(&output_path).await.map_err(|e| {
            AivyxError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to read edge-tts output: {e}"),
            ))
        })?;

        // Clean up temp file
        let _ = tokio::fs::remove_file(&output_path).await;

        if audio.is_empty() {
            return Err(AivyxError::Other(
                "edge-tts produced empty audio output".into(),
            ));
        }

        Ok(TtsOutput {
            audio,
            format: output_format,
            sample_rate: 24_000, // Edge TTS typically outputs 24kHz
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name() {
        let provider = EdgeTtsProvider::new();
        assert_eq!(provider.name(), "edge-tts");
    }

    #[test]
    fn default_constructor() {
        let provider = EdgeTtsProvider::default();
        assert_eq!(provider.name(), "edge-tts");
    }
}
