//! Conversation compression to manage context window limits.
//!
//! When a conversation approaches the LLM's context window limit,
//! older messages are summarized via an LLM call and replaced with
//! a compact summary message.

use aivyx_core::Result;
use aivyx_llm::{ChatMessage, ChatRequest, LlmProvider, Role, TokenUsage};

/// Result of a compression attempt, including token usage if an LLM call was made.
pub struct CompressionResult {
    /// The (possibly compressed) message list.
    pub messages: Vec<ChatMessage>,
    /// Token usage from the summarization LLM call, if compression occurred.
    pub usage: Option<TokenUsage>,
}

/// Compress a conversation if it exceeds the context window threshold.
///
/// If the estimated token count of `messages` exceeds `threshold_pct` of
/// `context_window`, the older 60% of messages are summarized via an LLM
/// call and replaced with a single system-role summary message.
///
/// Returns the (possibly compressed) message list and token usage.
pub async fn compress_conversation(
    provider: &dyn LlmProvider,
    messages: &[ChatMessage],
    context_window: u32,
    threshold_pct: f32,
) -> Result<CompressionResult> {
    // Only attempt compression if there are enough messages
    if messages.len() <= 4 {
        return Ok(CompressionResult {
            messages: messages.to_vec(),
            usage: None,
        });
    }

    // Estimate current token usage
    let estimated_tokens: u32 = messages.iter().map(|m| m.estimate_tokens()).sum();
    let threshold = (context_window as f32 * threshold_pct) as u32;

    if estimated_tokens < threshold {
        return Ok(CompressionResult {
            messages: messages.to_vec(),
            usage: None,
        });
    }

    // Split: older 60% -> summarize, recent 40% -> keep
    let split_point = (messages.len() * 3) / 5; // 60%
    let (old_messages, recent_messages) = messages.split_at(split_point);

    // Build the summary request
    let summary_content: String = old_messages
        .iter()
        .map(|m| format!("{}: {}", role_label(&m.role), &m.content))
        .collect::<Vec<_>>()
        .join("\n");

    let summary_request = ChatRequest {
        system_prompt: Some(
            "Summarize the following conversation concisely in 3-5 bullet points. \
             Preserve key facts, decisions, and context needed for continuing the conversation. \
             Be brief."
                .into(),
        ),
        messages: vec![ChatMessage::user(summary_content)],
        tools: vec![],
        model: None,
        max_tokens: 500,
    };

    let response = provider.chat(&summary_request).await?;
    let usage = response.usage;

    // Build compressed conversation: summary + recent messages
    let mut compressed = Vec::with_capacity(1 + recent_messages.len());
    compressed.push(ChatMessage::user(format!(
        "[Conversation summary]\n{}",
        response.message.content
    )));
    compressed.extend_from_slice(recent_messages);

    Ok(CompressionResult {
        messages: compressed,
        usage: Some(usage),
    })
}

/// Return a human-readable label for a message role.
fn role_label(role: &Role) -> &'static str {
    match role {
        Role::System => "System",
        Role::User => "User",
        Role::Assistant => "Assistant",
        Role::Tool => "Tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_llm::{ChatResponse, StopReason, TokenUsage};
    use async_trait::async_trait;

    struct MockProvider {
        response: String,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat(&self, _request: &ChatRequest) -> aivyx_core::Result<ChatResponse> {
            Ok(ChatResponse {
                message: ChatMessage::assistant(&self.response),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 10,
                },
                stop_reason: StopReason::EndTurn,
            })
        }
    }

    #[tokio::test]
    async fn compression_below_threshold_no_op() {
        let provider = MockProvider {
            response: "summary".into(),
        };
        let messages = vec![
            ChatMessage::user("Hello"),
            ChatMessage::assistant("Hi there!"),
        ];
        let result = compress_conversation(&provider, &messages, 200_000, 0.8)
            .await
            .unwrap();
        assert_eq!(result.messages.len(), 2); // unchanged - too few messages
        assert!(result.usage.is_none());
    }

    #[tokio::test]
    async fn compression_above_threshold_summarizes() {
        let provider = MockProvider {
            response: "- User discussed Rust\n- Asked about async".into(),
        };
        // Create enough messages to exceed threshold with a small context window
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(ChatMessage::user(format!(
                "Message number {i} with some content to fill tokens"
            )));
            messages.push(ChatMessage::assistant(format!(
                "Response {i} with details about the topic"
            )));
        }
        // Use a tiny context window so threshold is easily exceeded
        let result = compress_conversation(&provider, &messages, 100, 0.1)
            .await
            .unwrap();
        // Should be compressed: 1 summary + 40% of 40 = 16 recent
        assert!(result.messages.len() < messages.len());
        assert!(result.messages[0]
            .content
            .text()
            .contains("[Conversation summary]"));
        assert!(result.usage.is_some());
    }

    #[tokio::test]
    async fn compression_few_messages_no_op() {
        let provider = MockProvider {
            response: "summary".into(),
        };
        let messages = vec![
            ChatMessage::user("A"),
            ChatMessage::assistant("B"),
            ChatMessage::user("C"),
            ChatMessage::assistant("D"),
        ];
        // Exactly 4 messages should not compress (threshold is <= 4)
        let result = compress_conversation(&provider, &messages, 10, 0.1)
            .await
            .unwrap();
        assert_eq!(result.messages.len(), 4);
        assert!(result.usage.is_none());
    }

    #[test]
    fn role_label_coverage() {
        assert_eq!(role_label(&Role::System), "System");
        assert_eq!(role_label(&Role::User), "User");
        assert_eq!(role_label(&Role::Assistant), "Assistant");
        assert_eq!(role_label(&Role::Tool), "Tool");
    }
}
