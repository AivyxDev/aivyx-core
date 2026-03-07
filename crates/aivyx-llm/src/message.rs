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

// ---------------------------------------------------------------------------
// Multimodal content types
// ---------------------------------------------------------------------------

/// Message content — either plain text or a sequence of typed blocks.
///
/// Uses `#[serde(untagged)]` so bare JSON strings deserialize as `Text`,
/// preserving backwards compatibility with persisted sessions that stored
/// `"content": "hello"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// Plain text content (backwards-compatible with `String`).
    Text(String),
    /// Multimodal content blocks (text, images, etc.).
    Blocks(Vec<ContentBlock>),
}

impl Content {
    /// Extract the text portion of this content.
    ///
    /// For `Text`, returns the string directly. For `Blocks`, concatenates
    /// all text blocks separated by newlines.
    pub fn text(&self) -> &str {
        match self {
            Content::Text(s) => s,
            // For blocks we can't return a &str to a computed value,
            // so return empty if no single text block exists.
            // Use `to_text()` for the owned concatenation.
            Content::Blocks(blocks) => {
                // Fast path: return first text block
                for block in blocks {
                    if let ContentBlock::Text { text } = block {
                        return text;
                    }
                }
                ""
            }
        }
    }

    /// Owned text extraction — concatenates all text blocks.
    pub fn to_text(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Whether this content has no meaningful payload.
    pub fn is_empty(&self) -> bool {
        match self {
            Content::Text(s) => s.is_empty(),
            Content::Blocks(blocks) => blocks.is_empty(),
        }
    }

    /// Whether this content contains any image blocks.
    pub fn has_images(&self) -> bool {
        match self {
            Content::Text(_) => false,
            Content::Blocks(blocks) => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
        }
    }

    /// Extract all image sources from the content.
    pub fn image_sources(&self) -> Vec<&ImageSource> {
        match self {
            Content::Text(_) => Vec::new(),
            Content::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Image { source } => Some(source),
                    _ => None,
                })
                .collect(),
        }
    }

    /// Construct multimodal content from text and images.
    pub fn from_text_and_images(text: String, images: Vec<ImageSource>) -> Self {
        let mut blocks = Vec::with_capacity(1 + images.len());
        if !text.is_empty() {
            blocks.push(ContentBlock::Text { text });
        }
        for source in images {
            blocks.push(ContentBlock::Image { source });
        }
        Content::Blocks(blocks)
    }
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        Content::Text(s)
    }
}

impl From<&str> for Content {
    fn from(s: &str) -> Self {
        Content::Text(s.to_string())
    }
}

impl PartialEq<str> for Content {
    fn eq(&self, other: &str) -> bool {
        match self {
            Content::Text(s) => s == other,
            Content::Blocks(_) => self.to_text() == other,
        }
    }
}

impl PartialEq<&str> for Content {
    fn eq(&self, other: &&str) -> bool {
        self.eq(*other)
    }
}

impl std::fmt::Display for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Content::Text(s) => write!(f, "{s}"),
            Content::Blocks(_) => write!(f, "{}", self.to_text()),
        }
    }
}

/// A single content block within a multimodal message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content.
    Text { text: String },
    /// Image content with source data.
    Image { source: ImageSource },
}

/// Source data for an image content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Base64-encoded image data.
    Base64 { media_type: String, data: String },
    /// Image accessible via URL.
    Url { url: String },
}

// ---------------------------------------------------------------------------
// ChatMessage
// ---------------------------------------------------------------------------

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Content,
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
    /// plus overhead for role metadata and tool calls. Image blocks are
    /// estimated at ~1000 tokens each (typical vision model accounting).
    pub fn estimate_tokens(&self) -> u32 {
        let text_tokens = match &self.content {
            Content::Text(s) => (s.len() as u32).saturating_add(3) / 4,
            Content::Blocks(blocks) => {
                let mut tokens = 0u32;
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => {
                            tokens =
                                tokens.saturating_add((text.len() as u32).saturating_add(3) / 4);
                        }
                        ContentBlock::Image { .. } => {
                            tokens = tokens.saturating_add(1000);
                        }
                    }
                }
                tokens
            }
        };
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
            content: Content::Text(content.into()),
            tool_calls: Vec::new(),
            tool_result: None,
        }
    }

    /// Create a user message with text and images.
    pub fn user_with_images(text: impl Into<String>, images: Vec<ImageSource>) -> Self {
        Self {
            role: Role::User,
            content: Content::from_text_and_images(text.into(), images),
            tool_calls: Vec::new(),
            tool_result: None,
        }
    }

    /// Create a user message with multimodal content.
    pub fn user_multimodal(content: Content) -> Self {
        Self {
            role: Role::User,
            content,
            tool_calls: Vec::new(),
            tool_result: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Content::Text(content.into()),
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
            content: Content::Text(content.into()),
            tool_calls,
            tool_result: None,
        }
    }

    /// Create a tool result message.
    pub fn tool(result: ToolResult) -> Self {
        Self {
            role: Role::Tool,
            content: Content::Text(String::new()),
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
    fn estimate_tokens_with_image() {
        let msg = ChatMessage::user_with_images(
            "describe this",
            vec![ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "abc".into(),
            }],
        );
        let tokens = msg.estimate_tokens();
        // ~3 text tokens + 1000 image tokens + 4 overhead ≈ 1007
        assert!(tokens >= 1000);
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

    // --- Content type tests ---

    #[test]
    fn content_text_serde_roundtrip() {
        // Bare string → Content::Text
        let json = "\"hello world\"";
        let content: Content = serde_json::from_str(json).unwrap();
        assert_eq!(content, "hello world");
        assert_eq!(content.text(), "hello world");

        // Serializes back to bare string
        let serialized = serde_json::to_string(&content).unwrap();
        assert_eq!(serialized, "\"hello world\"");
    }

    #[test]
    fn content_blocks_serde_roundtrip() {
        let content = Content::from_text_and_images(
            "look at this".into(),
            vec![ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "iVBOR...".into(),
            }],
        );

        let json = serde_json::to_string(&content).unwrap();
        let parsed: Content = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.text(), "look at this");
        assert!(parsed.has_images());
        assert_eq!(parsed.image_sources().len(), 1);
    }

    #[test]
    fn content_from_string() {
        let content: Content = "test".into();
        assert_eq!(content, "test");
        assert!(!content.has_images());
        assert!(!content.is_empty());
    }

    #[test]
    fn content_empty() {
        let content = Content::Text(String::new());
        assert!(content.is_empty());

        let content = Content::Blocks(vec![]);
        assert!(content.is_empty());
    }

    #[test]
    fn content_display() {
        let content = Content::Text("hello".into());
        assert_eq!(format!("{content}"), "hello");
    }

    #[test]
    fn user_with_images_constructor() {
        let msg = ChatMessage::user_with_images(
            "what is this?",
            vec![ImageSource::Url {
                url: "https://example.com/img.png".into(),
            }],
        );
        assert_eq!(msg.role, Role::User);
        assert!(msg.content.has_images());
        assert_eq!(msg.content.text(), "what is this?");
    }

    #[test]
    fn backward_compat_chat_message_deserialization() {
        // Simulate a persisted session with the old String content format
        let json = r#"{"role":"user","content":"hello"}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "hello");
        assert_eq!(msg.content.text(), "hello");
    }
}
