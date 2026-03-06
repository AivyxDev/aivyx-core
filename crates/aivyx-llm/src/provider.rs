use aivyx_core::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::message::{ChatMessage, ChatRequest, ChatResponse, StopReason, TokenUsage};

/// Events emitted during streaming LLM responses.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of generated text.
    TextDelta(String),
    /// The stream has finished. Contains the final usage stats, stop reason,
    /// and the complete assembled message.
    Done {
        usage: TokenUsage,
        stop_reason: StopReason,
        message: crate::message::ChatMessage,
    },
    /// An error occurred during streaming.
    Error(String),
}

/// Trait for LLM providers.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Human-readable name of this provider.
    fn name(&self) -> &str;

    /// Return the context window size (max input tokens) for this provider.
    ///
    /// Defaults to 200,000 tokens (safe for most modern models).
    /// Override in provider implementations for model-specific limits.
    fn context_window(&self) -> u32 {
        200_000
    }

    /// Send a chat request and receive a response.
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse>;

    /// Check if the provider is reachable and functioning.
    ///
    /// Default implementation sends a minimal "ping" request. Providers may
    /// override with a lighter check (e.g., a models endpoint).
    async fn health_check(&self) -> Result<()> {
        let request = ChatRequest {
            system_prompt: None,
            messages: vec![ChatMessage::user("ping")],
            tools: vec![],
            model: None,
            max_tokens: 1,
        };
        self.chat(&request).await.map(|_| ())
    }

    /// Stream a chat response, sending events through the provided channel.
    ///
    /// Default implementation falls back to non-streaming `chat()` and sends
    /// the complete response as a single `TextDelta` followed by `Done`.
    async fn chat_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let response = self.chat(request).await?;

        if !response.message.content.is_empty() {
            let _ = tx
                .send(StreamEvent::TextDelta(response.message.content.clone()))
                .await;
        }

        let _ = tx
            .send(StreamEvent::Done {
                usage: response.usage,
                stop_reason: response.stop_reason,
                message: response.message,
            })
            .await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ChatMessage, ChatResponse, TokenUsage};

    struct FakeProvider {
        response: ChatResponse,
    }

    #[async_trait]
    impl LlmProvider for FakeProvider {
        fn name(&self) -> &str {
            "fake"
        }
        async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn default_chat_stream_fallback_text() {
        let provider = FakeProvider {
            response: ChatResponse {
                message: ChatMessage::assistant("Hello!"),
                usage: TokenUsage {
                    input_tokens: 5,
                    output_tokens: 2,
                },
                stop_reason: StopReason::EndTurn,
            },
        };

        let (tx, mut rx) = mpsc::channel(16);
        let request = ChatRequest {
            system_prompt: None,
            messages: vec![ChatMessage::user("Hi")],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };

        provider.chat_stream(&request, tx).await.unwrap();

        // Should get TextDelta then Done
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, StreamEvent::TextDelta(ref t) if t == "Hello!"));

        let second = rx.recv().await.unwrap();
        assert!(matches!(
            second,
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn,
                ..
            }
        ));

        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn default_chat_stream_fallback_empty_content() {
        let provider = FakeProvider {
            response: ChatResponse {
                message: ChatMessage::assistant(""),
                usage: TokenUsage {
                    input_tokens: 5,
                    output_tokens: 0,
                },
                stop_reason: StopReason::EndTurn,
            },
        };

        let (tx, mut rx) = mpsc::channel(16);
        let request = ChatRequest {
            system_prompt: None,
            messages: vec![ChatMessage::user("Hi")],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };

        provider.chat_stream(&request, tx).await.unwrap();

        // Empty content should skip TextDelta, go straight to Done
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, StreamEvent::Done { .. }));

        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn health_check_default_impl() {
        let provider = FakeProvider {
            response: ChatResponse {
                message: ChatMessage::assistant("pong"),
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                stop_reason: StopReason::EndTurn,
            },
        };
        // Default health_check sends a minimal chat request
        provider.health_check().await.unwrap();
    }

    #[test]
    fn context_window_default() {
        let provider = FakeProvider {
            response: ChatResponse {
                message: ChatMessage::assistant(""),
                usage: TokenUsage::default(),
                stop_reason: StopReason::EndTurn,
            },
        };
        assert_eq!(provider.context_window(), 200_000);
    }
}
