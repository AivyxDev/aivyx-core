use serde::{Deserialize, Serialize};

/// The role of a message participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Tool calls requested by the assistant (only present when role == Assistant).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Tool result (only present when role == Tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,
}

/// A tool invocation requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned ID for this tool call.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON arguments for the tool.
    pub arguments: serde_json::Value,
}

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool_call ID this result corresponds to.
    pub tool_call_id: String,
    /// JSON output from the tool.
    pub content: serde_json::Value,
    /// Whether the tool execution resulted in an error.
    #[serde(default)]
    pub is_error: bool,
}

/// A request to the LLM.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// System prompt (the agent's "soul").
    pub system_prompt: Option<String>,
    /// Conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Available tool definitions.
    pub tools: Vec<serde_json::Value>,
    /// Model to use (overrides provider default if set).
    pub model: Option<String>,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
}

impl ChatRequest {
    /// Estimate the total input token count for this request.
    ///
    /// Sums system prompt, messages, and tool definitions using the
    /// ~4 chars/token heuristic.
    pub fn estimate_input_tokens(&self) -> u32 {
        let system = self
            .system_prompt
            .as_ref()
            .map(|s| (s.len() as u32).saturating_add(3) / 4)
            .unwrap_or(0);
        let messages: u32 = self.messages.iter().map(|m| m.estimate_tokens()).sum();
        let tools: u32 = self
            .tools
            .iter()
            .map(|t| (t.to_string().len() as u32).saturating_add(3) / 4)
            .sum();
        system.saturating_add(messages).saturating_add(tools)
    }
}

/// A response from the LLM.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// The assistant's response message.
    pub message: ChatMessage,
    /// Token usage statistics.
    pub usage: TokenUsage,
    /// Why the model stopped generating.
    pub stop_reason: StopReason,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Reason the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Normal end of response.
    EndTurn,
    /// Hit the max_tokens limit.
    MaxTokens,
    /// Model wants to use a tool.
    ToolUse,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::EndTurn => write!(f, "end_turn"),
            StopReason::MaxTokens => write!(f, "max_tokens"),
            StopReason::ToolUse => write!(f, "tool_use"),
        }
    }
}

impl ChatMessage {
    /// Estimate the token count for this message using a heuristic.
    ///
    /// Uses the industry-standard approximation of ~4 characters per token,
    /// plus overhead for role metadata and tool calls.
    pub fn estimate_tokens(&self) -> u32 {
        let text_tokens = (self.content.len() as u32).saturating_add(3) / 4;
        let tool_tokens: u32 = self
            .tool_calls
            .iter()
            .map(|tc| (tc.arguments.to_string().len() as u32).saturating_add(3) / 4 + 10)
            .sum();
        let tool_result_tokens: u32 = self
            .tool_result
            .as_ref()
            .map(|tr| (tr.content.to_string().len() as u32).saturating_add(3) / 4 + 5)
            .unwrap_or(0);
        text_tokens
            .saturating_add(tool_tokens)
            .saturating_add(tool_result_tokens)
            .saturating_add(4) // role overhead
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_result: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_result: None,
        }
    }

    /// Create an assistant message with tool calls.
    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_result: None,
        }
    }

    /// Create a tool result message.
    pub fn tool(result: ToolResult) -> Self {
        Self {
            role: Role::Tool,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_result: Some(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message() {
        let msg = ChatMessage::user("hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "hello");
        assert!(msg.tool_calls.is_empty());
    }

    #[test]
    fn assistant_message() {
        let msg = ChatMessage::assistant("world");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, "world");
    }

    #[test]
    fn tool_result_message() {
        let result = ToolResult {
            tool_call_id: "tc_1".into(),
            content: serde_json::json!({"ok": true}),
            is_error: false,
        };
        let msg = ChatMessage::tool(result);
        assert_eq!(msg.role, Role::Tool);
        assert!(msg.tool_result.is_some());
    }

    #[test]
    fn role_serde_roundtrip() {
        let role = Role::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"assistant\"");
        let parsed: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Role::Assistant);
    }

    #[test]
    fn estimate_tokens_empty() {
        let msg = ChatMessage::user("");
        // Empty content: 0 text + 4 overhead = 4
        assert_eq!(msg.estimate_tokens(), 4);
    }

    #[test]
    fn estimate_tokens_with_content() {
        // 20 chars → ~5 tokens + 4 overhead = ~9
        let msg = ChatMessage::user("Hello, this is a tes");
        let tokens = msg.estimate_tokens();
        assert!(tokens >= 8 && tokens <= 10);
    }

    #[test]
    fn estimate_input_tokens_request() {
        let request = ChatRequest {
            system_prompt: Some("You are helpful.".into()),
            messages: vec![ChatMessage::user("Hi"), ChatMessage::assistant("Hello!")],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };
        let tokens = request.estimate_input_tokens();
        // System (~4) + user msg (~5) + assistant msg (~6) = ~15
        assert!(tokens > 10);
        assert!(tokens < 50);
    }

    #[test]
    fn stop_reason_display() {
        assert_eq!(StopReason::EndTurn.to_string(), "end_turn");
        assert_eq!(StopReason::MaxTokens.to_string(), "max_tokens");
        assert_eq!(StopReason::ToolUse.to_string(), "tool_use");
    }
}
