use async_trait::async_trait;
use tokio::sync::mpsc;

use aivyx_core::Result;

use crate::message::{ChatRequest, ChatResponse};
use crate::openai_compat::OpenAICompatibleProvider;
use crate::provider::{LlmProvider, StreamEvent};

/// Ollama provider using the OpenAI-compatible chat completions endpoint.
///
/// Thin wrapper around [`OpenAICompatibleProvider`] with no authentication
/// and a configurable base URL (defaults to `http://localhost:11434`).
pub struct OllamaProvider {
    inner: OpenAICompatibleProvider,
}

impl OllamaProvider {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(None, model, base_url, "ollama".into(), 60),
        }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        self.inner.chat(request).await
    }

    async fn chat_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        self.inner.chat_stream(request, tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name() {
        let p = OllamaProvider::new("http://localhost:11434".into(), "llama3".into());
        assert_eq!(p.name(), "ollama");
    }
}
