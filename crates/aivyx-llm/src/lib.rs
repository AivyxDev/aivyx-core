//! LLM provider abstraction for the aivyx framework.
//!
//! Provides message types, provider traits, and concrete implementations for
//! Claude, OpenAI, and Ollama APIs. Also includes STT and TTS provider
//! abstractions for voice features.

pub mod claude;
pub mod embedding;
pub mod factory;
pub mod message;
pub mod ollama;
pub mod openai;
pub mod openai_compat;
pub mod provider;
pub mod stt;
pub mod stt_ollama;
pub mod stt_openai;
pub mod tts;
pub mod tts_edge;
pub mod tts_openai;

pub use claude::ClaudeProvider;
pub use embedding::{Embedding, EmbeddingProvider, create_embedding_provider};
pub use factory::{create_provider, create_stt_provider, create_tts_provider};
pub use message::{
    ChatMessage, ChatRequest, ChatResponse, Content, ContentBlock, ImageSource, Role, StopReason,
    TokenUsage, ToolCall, ToolResult,
};
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub use openai_compat::OpenAICompatibleProvider;
pub use provider::{LlmProvider, StreamEvent};
pub use stt::{AudioFormat, SttProvider, TranscriptionResult};
pub use stt_ollama::OllamaSttProvider;
pub use stt_openai::OpenAiSttProvider;
pub use tts::{TtsAudioFormat, TtsOptions, TtsOutput, TtsProvider};
pub use tts_edge::EdgeTtsProvider;
pub use tts_openai::OpenAiTtsProvider;
