use async_trait::async_trait;
use secrecy::SecretString;
use tokio::sync::mpsc;

use aivyx_core::Result;

use crate::message::{ChatRequest, ChatResponse};
use crate::openai_compat::OpenAICompatibleProvider;
use crate::provider::{LlmProvider, StreamEvent};

/// OpenAI Chat Completions API provider.
///
/// Thin wrapper around [`OpenAICompatibleProvider`] with the standard OpenAI
/// endpoint and authentication pre-configured.
pub struct OpenAIProvider {
    inner: OpenAICompatibleProvider,
}

impl OpenAIProvider {
    pub fn new(api_key: SecretString, model: String) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                Some(api_key),
                model,
                "https://api.openai.com".into(),
                "openai".into(),
                120,
            ),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
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
        let p = OpenAIProvider::new(SecretString::from("test-key".to_string()), "gpt-4o".into());
        assert_eq!(p.name(), "openai");
    }
}
