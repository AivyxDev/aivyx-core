//! Complexity classifier for smart model routing.
//!
//! Classifies a [`ChatRequest`] into a complexity level using heuristic
//! signals: estimated token count, conversation depth, tool availability,
//! image presence, and user message length. The classification determines
//! which LLM provider handles the request — cheaper models for simple
//! queries, more capable models for complex reasoning chains.
//!
//! This is a pure function with no side effects — suitable for use in
//! aivyx-core without any engine-side dependencies.

use serde::{Deserialize, Serialize};

use crate::message::{ChatRequest, Role};

/// Complexity level for model routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComplexityLevel {
    /// Short Q&A, lookups, simple formatting — cheapest model suffices.
    Simple,
    /// Standard tool use, moderate context, multi-turn conversations.
    Medium,
    /// Multi-step reasoning, large context windows, complex tool orchestration.
    Complex,
}

impl std::fmt::Display for ComplexityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComplexityLevel::Simple => write!(f, "Simple"),
            ComplexityLevel::Medium => write!(f, "Medium"),
            ComplexityLevel::Complex => write!(f, "Complex"),
        }
    }
}

/// Heuristic signals extracted from a [`ChatRequest`].
///
/// Exposed publicly for logging and debugging — callers can inspect
/// what drove a particular classification decision.
#[derive(Debug, Clone)]
pub struct ComplexitySignals {
    /// Estimated total input tokens (from `ChatRequest::estimate_input_tokens()`).
    pub estimated_input_tokens: u32,
    /// Number of messages in the conversation history.
    pub message_count: usize,
    /// Number of available tool definitions.
    pub tool_count: usize,
    /// Whether the request contains image content.
    pub has_images: bool,
    /// Character length of the last user message.
    pub last_user_message_len: usize,
    /// Computed heuristic score.
    pub score: u32,
}

/// Extract heuristic signals from a chat request.
pub fn extract_signals(request: &ChatRequest) -> ComplexitySignals {
    let estimated_input_tokens = request.estimate_input_tokens();
    let message_count = request.messages.len();
    let tool_count = request.tools.len();

    let has_images = request.messages.iter().any(|m| m.content.has_images());

    let last_user_message_len = request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.to_text().len())
        .unwrap_or(0);

    let mut score: u32 = 0;

    // Token count thresholds
    if estimated_input_tokens > 2000 {
        score += 2;
    }
    if estimated_input_tokens > 8000 {
        score += 3;
    }

    // Conversation depth
    if message_count > 4 {
        score += 1;
    }
    if message_count > 10 {
        score += 2;
    }

    // Tool availability (more tools = more complex orchestration potential)
    if tool_count > 3 {
        score += 1;
    }
    if tool_count > 8 {
        score += 2;
    }

    // Image presence (vision tasks are at least medium complexity)
    if has_images {
        score += 2;
    }

    // Last user message length
    if last_user_message_len > 500 {
        score += 1;
    }
    if last_user_message_len > 2000 {
        score += 2;
    }

    ComplexitySignals {
        estimated_input_tokens,
        message_count,
        tool_count,
        has_images,
        last_user_message_len,
        score,
    }
}

/// Classify a [`ChatRequest`] into a complexity level.
///
/// Uses heuristic scoring based on request characteristics. The score
/// ranges map to:
/// - **0–2** → `Simple` (short Q&A, basic formatting)
/// - **3–5** → `Medium` (tool use, moderate conversation depth)
/// - **6+**  → `Complex` (multi-step reasoning, large context, images)
pub fn classify(request: &ChatRequest) -> ComplexityLevel {
    let signals = extract_signals(request);
    match signals.score {
        0..=2 => ComplexityLevel::Simple,
        3..=5 => ComplexityLevel::Medium,
        _ => ComplexityLevel::Complex,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ChatMessage, Content};

    fn simple_request(user_msg: &str) -> ChatRequest {
        ChatRequest {
            system_prompt: Some("You are helpful.".into()),
            messages: vec![ChatMessage {
                role: Role::User,
                content: Content::Text(user_msg.into()),
                tool_calls: vec![],
                tool_result: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: 100,
        }
    }

    fn request_with_messages(count: usize) -> ChatRequest {
        let mut messages = Vec::with_capacity(count);
        for i in 0..count {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            messages.push(ChatMessage {
                role,
                content: Content::Text("Hello, this is a message.".into()),
                tool_calls: vec![],
                tool_result: None,
            });
        }
        ChatRequest {
            system_prompt: Some("System prompt.".into()),
            messages,
            tools: vec![],
            model: None,
            max_tokens: 100,
        }
    }

    fn request_with_tools(tool_count: usize) -> ChatRequest {
        let tools: Vec<serde_json::Value> = (0..tool_count)
            .map(|i| {
                serde_json::json!({
                    "name": format!("tool_{i}"),
                    "description": "A test tool for classification",
                    "input_schema": {"type": "object"}
                })
            })
            .collect();
        ChatRequest {
            system_prompt: Some("You are helpful.".into()),
            messages: vec![ChatMessage {
                role: Role::User,
                content: Content::Text("Use a tool.".into()),
                tool_calls: vec![],
                tool_result: None,
            }],
            tools,
            model: None,
            max_tokens: 100,
        }
    }

    #[test]
    fn short_message_is_simple() {
        let req = simple_request("What time is it?");
        assert_eq!(classify(&req), ComplexityLevel::Simple);
    }

    #[test]
    fn single_message_no_tools_is_simple() {
        let req = simple_request("Hello");
        let signals = extract_signals(&req);
        assert!(signals.score <= 2);
        assert_eq!(classify(&req), ComplexityLevel::Simple);
    }

    #[test]
    fn many_messages_increases_complexity() {
        // 12 messages → score gets +1 (>4) and +2 (>10) = 3 → Medium
        let req = request_with_messages(12);
        assert_eq!(classify(&req), ComplexityLevel::Medium);
    }

    #[test]
    fn many_tools_increases_complexity() {
        // 10 tools → score gets +1 (>3) and +2 (>8) = 3 → Medium
        let req = request_with_tools(10);
        assert_eq!(classify(&req), ComplexityLevel::Medium);
    }

    #[test]
    fn long_user_message_increases_complexity() {
        // 600 chars → +1 for >500
        let long_msg = "x".repeat(600);
        let req = simple_request(&long_msg);
        let signals = extract_signals(&req);
        assert!(signals.last_user_message_len > 500);
        // Score: token estimate (~150 tokens from 600 chars) ≤ 2000, so no token bump
        // Just the +1 from message length → still Simple (score 1)
        assert_eq!(classify(&req), ComplexityLevel::Simple);
    }

    #[test]
    fn very_long_user_message_increases_complexity() {
        // 2500 chars → +1 (>500) + +2 (>2000) = 3 → Medium
        let long_msg = "x".repeat(2500);
        let req = simple_request(&long_msg);
        assert_eq!(classify(&req), ComplexityLevel::Medium);
    }

    #[test]
    fn high_tokens_is_complex() {
        // >8000 tokens (32000+ chars) → +2 (>2000) + +3 (>8000) = 5
        // Plus the long message bonus: +1 (>500) + +2 (>2000) = 3
        // Total: 8 → Complex
        let huge_msg = "word ".repeat(8000); // ~40000 chars ≈ 10000 tokens
        let req = simple_request(&huge_msg);
        assert_eq!(classify(&req), ComplexityLevel::Complex);
    }

    #[test]
    fn combined_signals_stack() {
        // 6 messages + 5 tools + 600 char message
        // messages: +1 (>4)
        // tools: +1 (>3)
        // user msg: +1 (>500)
        // Total: 3 → Medium
        let tools: Vec<serde_json::Value> = (0..5)
            .map(|i| serde_json::json!({"name": format!("t{i}"), "description": "d"}))
            .collect();
        let mut messages = Vec::new();
        for i in 0..5 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            messages.push(ChatMessage {
                role,
                content: Content::Text("Hello.".into()),
                tool_calls: vec![],
                tool_result: None,
            });
        }
        messages.push(ChatMessage {
            role: Role::User,
            content: Content::Text("x".repeat(600)),
            tool_calls: vec![],
            tool_result: None,
        });
        let req = ChatRequest {
            system_prompt: Some("System.".into()),
            messages,
            tools,
            model: None,
            max_tokens: 100,
        };
        assert_eq!(classify(&req), ComplexityLevel::Medium);
    }

    #[test]
    fn empty_request_is_simple() {
        let req = ChatRequest {
            system_prompt: None,
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };
        assert_eq!(classify(&req), ComplexityLevel::Simple);
    }

    #[test]
    fn complexity_level_display() {
        assert_eq!(format!("{}", ComplexityLevel::Simple), "Simple");
        assert_eq!(format!("{}", ComplexityLevel::Medium), "Medium");
        assert_eq!(format!("{}", ComplexityLevel::Complex), "Complex");
    }
}
