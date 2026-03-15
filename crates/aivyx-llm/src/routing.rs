//! Complexity-based model routing provider.
//!
//! [`RoutingProvider`] wraps multiple inner [`LlmProvider`] instances and
//! routes each request to the cheapest adequate model based on the
//! [`ComplexityLevel`] of the request. It implements `LlmProvider` itself,
//! making it transparent to the agent turn loop.
//!
//! # Provider chain
//!
//! ```text
//! CachingProvider(RoutingProvider(
//!     Simple  → provider_a,
//!     Medium  → provider_b,   // optional — falls back to default
//!     Complex → provider_c,   // optional — falls back to default
//!     default → primary_provider,
//! ))
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use aivyx_core::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::classifier::{ComplexityLevel, classify};
use crate::message::{ChatRequest, ChatResponse};
use crate::provider::{LlmProvider, StreamEvent};

/// Events emitted by the routing layer for observability.
#[derive(Debug, Clone)]
pub enum RoutingEvent {
    /// A request was routed to a specific provider based on complexity.
    Routed {
        /// The classified complexity level.
        complexity: ComplexityLevel,
        /// Name of the provider that handled the request.
        provider: String,
    },
}

/// Observer callback for routing events (audit logging, metrics, etc.).
pub type RoutingObserver = Arc<dyn Fn(RoutingEvent) + Send + Sync>;

/// LLM provider wrapper that routes requests to different providers
/// based on complexity classification.
///
/// Falls back to the default provider for any complexity level that
/// has no specific assignment.
pub struct RoutingProvider {
    /// Per-complexity-level providers.
    providers: HashMap<ComplexityLevel, Box<dyn LlmProvider>>,
    /// Fallback provider when no tier-specific one is configured.
    default_provider: Box<dyn LlmProvider>,
    /// Optional observer for routing events.
    observer: Option<RoutingObserver>,
}

impl RoutingProvider {
    /// Create a new routing provider.
    ///
    /// `default_provider` is used for any complexity level not present in
    /// `tier_providers`. At least one tier should be configured for routing
    /// to have any effect.
    pub fn new(
        default_provider: Box<dyn LlmProvider>,
        tier_providers: HashMap<ComplexityLevel, Box<dyn LlmProvider>>,
    ) -> Self {
        Self {
            providers: tier_providers,
            default_provider,
            observer: None,
        }
    }

    /// Attach an observer for routing events.
    pub fn with_observer(mut self, observer: RoutingObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Select the provider for a given complexity level.
    fn select(&self, level: ComplexityLevel) -> &dyn LlmProvider {
        self.providers
            .get(&level)
            .map(|p| p.as_ref())
            .unwrap_or(self.default_provider.as_ref())
    }

    /// Notify the observer about a routing decision.
    fn notify(&self, level: ComplexityLevel, provider_name: &str) {
        if let Some(ref observer) = self.observer {
            observer(RoutingEvent::Routed {
                complexity: level,
                provider: provider_name.to_string(),
            });
        }
    }
}

impl std::fmt::Debug for RoutingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingProvider")
            .field("default", &self.default_provider.name())
            .field("tiers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[async_trait]
impl LlmProvider for RoutingProvider {
    fn name(&self) -> &str {
        self.default_provider.name()
    }

    fn model_name(&self) -> &str {
        self.default_provider.model_name()
    }

    fn context_window(&self) -> u32 {
        // Return the minimum across all providers (safe bound).
        let default_cw = self.default_provider.context_window();
        self.providers
            .values()
            .map(|p| p.context_window())
            .fold(default_cw, |min, cw| min.min(cw))
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let level = classify(request);
        let provider = self.select(level);
        let name = provider.name().to_string();

        tracing::debug!(
            complexity = %level,
            provider = %name,
            "routing request"
        );

        self.notify(level, &name);
        provider.chat(request).await
    }

    async fn chat_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let level = classify(request);
        let provider = self.select(level);
        let name = provider.name().to_string();

        tracing::debug!(
            complexity = %level,
            provider = %name,
            "routing stream request"
        );

        self.notify(level, &name);
        provider.chat_stream(request, tx).await
    }

    async fn health_check(&self) -> Result<()> {
        // Check all providers — if any is down, report it.
        self.default_provider.health_check().await?;
        for provider in self.providers.values() {
            provider.health_check().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ChatMessage, ChatResponse, StopReason, TokenUsage};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A mock provider that counts how many times `chat()` was called.
    struct CountingProvider {
        name: &'static str,
        call_count: Arc<AtomicU32>,
    }

    impl CountingProvider {
        fn new(name: &'static str) -> (Self, Arc<AtomicU32>) {
            let count = Arc::new(AtomicU32::new(0));
            (
                Self {
                    name,
                    call_count: count.clone(),
                },
                count,
            )
        }
    }

    #[async_trait]
    impl LlmProvider for CountingProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn context_window(&self) -> u32 {
            if self.name == "small" { 8_000 } else { 200_000 }
        }

        async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(ChatResponse {
                message: ChatMessage::assistant("ok"),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                },
                stop_reason: StopReason::EndTurn,
            })
        }
    }

    fn simple_request() -> ChatRequest {
        ChatRequest {
            system_prompt: Some("Hi.".into()),
            messages: vec![ChatMessage::user("Hello")],
            tools: vec![],
            model: None,
            max_tokens: 100,
        }
    }

    fn complex_request() -> ChatRequest {
        let huge_msg = "word ".repeat(8000);
        ChatRequest {
            system_prompt: Some("System.".into()),
            messages: vec![ChatMessage::user(&huge_msg)],
            tools: vec![],
            model: None,
            max_tokens: 4096,
        }
    }

    #[tokio::test]
    async fn routes_simple_to_tier_provider() {
        let (default, default_count) = CountingProvider::new("default");
        let (simple, simple_count) = CountingProvider::new("simple");

        let mut tiers = HashMap::new();
        tiers.insert(
            ComplexityLevel::Simple,
            Box::new(simple) as Box<dyn LlmProvider>,
        );

        let router = RoutingProvider::new(Box::new(default), tiers);
        router.chat(&simple_request()).await.unwrap();

        assert_eq!(simple_count.load(Ordering::Relaxed), 1);
        assert_eq!(default_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn routes_complex_to_default_when_no_tier() {
        let (default, default_count) = CountingProvider::new("default");
        let (simple, simple_count) = CountingProvider::new("simple");

        let mut tiers = HashMap::new();
        tiers.insert(
            ComplexityLevel::Simple,
            Box::new(simple) as Box<dyn LlmProvider>,
        );

        let router = RoutingProvider::new(Box::new(default), tiers);
        router.chat(&complex_request()).await.unwrap();

        // Complex has no tier provider → falls back to default
        assert_eq!(default_count.load(Ordering::Relaxed), 1);
        assert_eq!(simple_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn routes_complex_to_tier_provider() {
        let (default, default_count) = CountingProvider::new("default");
        let (complex, complex_count) = CountingProvider::new("complex");

        let mut tiers = HashMap::new();
        tiers.insert(
            ComplexityLevel::Complex,
            Box::new(complex) as Box<dyn LlmProvider>,
        );

        let router = RoutingProvider::new(Box::new(default), tiers);
        router.chat(&complex_request()).await.unwrap();

        assert_eq!(complex_count.load(Ordering::Relaxed), 1);
        assert_eq!(default_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn observer_is_notified() {
        let (default, _) = CountingProvider::new("default");
        let (simple, _) = CountingProvider::new("simple");

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let obs_clone = observed.clone();

        let mut tiers = HashMap::new();
        tiers.insert(
            ComplexityLevel::Simple,
            Box::new(simple) as Box<dyn LlmProvider>,
        );

        let router = RoutingProvider::new(Box::new(default), tiers).with_observer(Arc::new(
            move |event: RoutingEvent| {
                if let RoutingEvent::Routed {
                    complexity,
                    provider,
                } = event
                {
                    obs_clone.lock().unwrap().push((complexity, provider));
                }
            },
        ));

        router.chat(&simple_request()).await.unwrap();

        let events = observed.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, ComplexityLevel::Simple);
        assert_eq!(events[0].1, "simple");
    }

    #[test]
    fn context_window_returns_minimum() {
        let (default, _) = CountingProvider::new("default"); // 200_000
        let (small, _) = CountingProvider::new("small"); // 8_000

        let mut tiers = HashMap::new();
        tiers.insert(
            ComplexityLevel::Simple,
            Box::new(small) as Box<dyn LlmProvider>,
        );

        let router = RoutingProvider::new(Box::new(default), tiers);
        assert_eq!(router.context_window(), 8_000);
    }

    #[test]
    fn name_delegates_to_default() {
        let (default, _) = CountingProvider::new("my-default");
        let router = RoutingProvider::new(Box::new(default), HashMap::new());
        assert_eq!(router.name(), "my-default");
    }

    #[tokio::test]
    async fn empty_tiers_always_uses_default() {
        let (default, default_count) = CountingProvider::new("default");
        let router = RoutingProvider::new(Box::new(default), HashMap::new());

        router.chat(&simple_request()).await.unwrap();
        router.chat(&complex_request()).await.unwrap();

        assert_eq!(default_count.load(Ordering::Relaxed), 2);
    }
}
