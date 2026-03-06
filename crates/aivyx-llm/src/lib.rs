//! LLM provider abstraction for the aivyx framework.
//!
//! Provides message types, provider traits, and concrete implementations for
//! Claude and Ollama (OpenAI-compatible) APIs.

pub mod claude;
pub mod embedding;
pub mod factory;
pub mod message;
pub mod ollama;
pub mod openai;
pub mod provider;

pub use claude::ClaudeProvider;
pub use embedding::{Embedding, EmbeddingProvider, create_embedding_provider};
pub use factory::create_provider;
pub use message::{
    ChatMessage, ChatRequest, ChatResponse, Role, StopReason, TokenUsage, ToolCall, ToolResult,
};
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub use provider::{LlmProvider, StreamEvent};
