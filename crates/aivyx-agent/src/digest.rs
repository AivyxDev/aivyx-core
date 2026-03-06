//! Daily digest generation.
//!
//! Runs a single LLM call with a digest-specific system prompt, returning a
//! concise morning briefing. The caller is responsible for building context
//! (memory, notifications) and storing the result as a notification if needed.

use aivyx_core::Result;
use aivyx_llm::{ChatMessage, ChatRequest, LlmProvider};

/// System prompt for digest generation.
const DIGEST_SYSTEM_PROMPT: &str = "\
You are a proactive personal assistant generating a concise daily digest. \
Synthesize what you know from the provided context to give the user a useful \
daily briefing. Be brief — 5-10 bullet points maximum. Focus on actionable \
items, recent activity, and anything the user should know about today.";

/// Generate a morning digest using the provided LLM provider.
///
/// Uses a direct LLM call (not a full agent turn loop) to avoid tool-use
/// loops. The caller is responsible for building the prompt with any relevant
/// memory or notification context.
///
/// # Arguments
///
/// * `provider` — The LLM provider to call.
/// * `prompt` — The user-facing prompt (e.g., recent activity summary).
/// * `system_context` — Optional additional context appended to the system
///   prompt (e.g., memory context, user profile).
pub async fn generate_digest(
    provider: &dyn LlmProvider,
    prompt: &str,
    system_context: Option<&str>,
) -> Result<String> {
    let system = match system_context {
        Some(ctx) => format!("{DIGEST_SYSTEM_PROMPT}\n\n{ctx}"),
        None => DIGEST_SYSTEM_PROMPT.to_string(),
    };

    let request = ChatRequest {
        system_prompt: Some(system),
        messages: vec![ChatMessage::user(prompt)],
        tools: vec![],
        model: None,
        max_tokens: 1024,
    };

    let response = provider.chat(&request).await?;
    Ok(response.message.content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_system_prompt_format() {
        let with_ctx = format!("{DIGEST_SYSTEM_PROMPT}\n\nExtra context");
        assert!(with_ctx.contains("daily digest"));
        assert!(with_ctx.contains("Extra context"));
    }
}
