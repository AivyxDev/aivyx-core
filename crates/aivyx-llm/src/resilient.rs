//! Resilient LLM provider with circuit breakers and automatic failover.
//!
//! Wraps a primary provider and zero or more fallbacks, each with its own
//! [`CircuitBreaker`]. When the primary's circuit opens (consecutive failures
//! exceed the threshold), requests are transparently routed to the next
//! available fallback.
//!
//! Implements [`LlmProvider`] so it is transparent to the agent — the agent
//! sees a single provider that happens to be resilient.
//!
//! # Architecture
//!
//! ```text
//! Agent
//!   └─ ResilientProvider (implements LlmProvider)
//!        ├─ CircuitBreaker[primary]   → Box<dyn LlmProvider>
//!        ├─ CircuitBreaker[fallback1] → Box<dyn LlmProvider>
//!        └─ CircuitBreaker[fallback2] → Box<dyn LlmProvider>
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use aivyx_core::{AivyxError, Result};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use crate::message::{ChatRequest, ChatResponse};
use crate::provider::{LlmProvider, StreamEvent};

/// Events emitted during provider failover for observability.
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    /// A provider's circuit breaker opened (provider considered down).
    CircuitOpened { provider: String, failures: u32 },
    /// A provider's circuit breaker closed (provider recovered).
    CircuitClosed { provider: String },
    /// Traffic was routed from a failed provider to a fallback.
    FailoverActivated { from: String, to: String },
    /// All configured providers are unavailable.
    AllProvidersDown,
}

/// Observer callback for provider failover events.
///
/// Injected by the agent layer to bridge provider events with the audit log
/// without creating a direct dependency from `aivyx-llm` to `aivyx-audit`.
pub type FailoverObserver = Arc<dyn Fn(ProviderEvent) + Send + Sync>;

/// A provider entry with its circuit breaker.
struct ProviderEntry {
    provider: Box<dyn LlmProvider>,
    breaker: CircuitBreaker,
    name: String,
}

/// Resilient LLM provider with circuit breakers and automatic failover.
///
/// When the primary provider fails repeatedly, requests are transparently
/// routed to fallback providers. Each provider has its own circuit breaker
/// that tracks failures independently.
pub struct ResilientProvider {
    /// All providers in priority order (index 0 = primary).
    entries: Vec<ProviderEntry>,
    /// Optional observer for audit/observability of failover events.
    observer: Option<FailoverObserver>,
}

impl ResilientProvider {
    /// Create a resilient provider with a single primary (no fallbacks yet).
    pub fn new(
        primary: Box<dyn LlmProvider>,
        primary_name: String,
        breaker_config: CircuitBreakerConfig,
    ) -> Self {
        Self {
            entries: vec![ProviderEntry {
                provider: primary,
                breaker: CircuitBreaker::new(breaker_config),
                name: primary_name,
            }],
            observer: None,
        }
    }

    /// Add a fallback provider (tried after all higher-priority providers).
    pub fn with_fallback(
        mut self,
        provider: Box<dyn LlmProvider>,
        name: String,
        breaker_config: CircuitBreakerConfig,
    ) -> Self {
        self.entries.push(ProviderEntry {
            provider,
            breaker: CircuitBreaker::new(breaker_config),
            name,
        });
        self
    }

    /// Attach an observer for failover events.
    pub fn with_observer(mut self, observer: FailoverObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Emit a provider event to the observer, if set.
    fn emit(&self, event: ProviderEvent) {
        if let Some(ref obs) = self.observer {
            obs(event);
        }
    }

    /// Return the name of the first available provider, or "unavailable".
    fn active_name(&self) -> &str {
        for entry in &self.entries {
            if entry.breaker.state() != CircuitState::Open {
                return &entry.name;
            }
        }
        "unavailable"
    }
}

#[async_trait]
impl LlmProvider for ResilientProvider {
    fn name(&self) -> &str {
        self.active_name()
    }

    fn model_name(&self) -> &str {
        for entry in &self.entries {
            if entry.breaker.state() != CircuitState::Open {
                return entry.provider.model_name();
            }
        }
        "unavailable"
    }

    fn context_window(&self) -> u32 {
        for entry in &self.entries {
            if entry.breaker.state() != CircuitState::Open {
                return entry.provider.context_window();
            }
        }
        200_000
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let mut last_error = None;
        let mut failed_provider = None;

        for entry in &self.entries {
            if !entry.breaker.can_execute() {
                continue;
            }

            match entry.provider.chat(request).await {
                Ok(response) => {
                    let was_half_open = entry.breaker.state() == CircuitState::HalfOpen;
                    entry.breaker.record_success();

                    // If circuit just closed (recovered), emit event.
                    if was_half_open && entry.breaker.state() == CircuitState::Closed {
                        self.emit(ProviderEvent::CircuitClosed {
                            provider: entry.name.clone(),
                        });
                    }

                    // If we failed over from a previous provider, emit event.
                    if let Some(from) = failed_provider {
                        self.emit(ProviderEvent::FailoverActivated {
                            from,
                            to: entry.name.clone(),
                        });
                    }

                    return Ok(response);
                }
                Err(e) => {
                    let just_opened = entry.breaker.record_failure();

                    if just_opened {
                        self.emit(ProviderEvent::CircuitOpened {
                            provider: entry.name.clone(),
                            failures: entry.breaker.failure_count(),
                        });
                    }

                    tracing::warn!(
                        provider = %entry.name,
                        error = %e,
                        "Provider call failed, trying next"
                    );

                    if failed_provider.is_none() {
                        failed_provider = Some(entry.name.clone());
                    }
                    last_error = Some(e);
                }
            }
        }

        self.emit(ProviderEvent::AllProvidersDown);

        match last_error {
            Some(e) => Err(e),
            None => Err(AivyxError::LlmProvider(
                "all providers unavailable (circuits open)".into(),
            )),
        }
    }

    async fn chat_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let mut last_error = None;

        for entry in &self.entries {
            if !entry.breaker.can_execute() {
                continue;
            }

            match entry.provider.chat_stream(request, tx.clone()).await {
                Ok(()) => {
                    let was_half_open = entry.breaker.state() == CircuitState::HalfOpen;
                    entry.breaker.record_success();

                    if was_half_open && entry.breaker.state() == CircuitState::Closed {
                        self.emit(ProviderEvent::CircuitClosed {
                            provider: entry.name.clone(),
                        });
                    }

                    return Ok(());
                }
                Err(e) => {
                    let just_opened = entry.breaker.record_failure();

                    if just_opened {
                        self.emit(ProviderEvent::CircuitOpened {
                            provider: entry.name.clone(),
                            failures: entry.breaker.failure_count(),
                        });
                    }

                    tracing::warn!(
                        provider = %entry.name,
                        error = %e,
                        "Provider stream failed, trying next"
                    );

                    last_error = Some(e);
                }
            }
        }

        self.emit(ProviderEvent::AllProvidersDown);

        match last_error {
            Some(e) => Err(e),
            None => Err(AivyxError::LlmProvider(
                "all providers unavailable (circuits open)".into(),
            )),
        }
    }

    async fn health_check(&self) -> Result<()> {
        // Return Ok if any provider is healthy.
        for entry in &self.entries {
            if entry.provider.health_check().await.is_ok() {
                return Ok(());
            }
        }
        Err(AivyxError::LlmProvider(
            "all providers failed health check".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ChatMessage, ChatResponse, StopReason, TokenUsage};
    use std::sync::Mutex as StdMutex;

    /// A mock provider that returns predefined results.
    struct MockProvider {
        name: String,
        results: StdMutex<Vec<Result<ChatResponse>>>,
    }

    impl MockProvider {
        fn succeeding(name: &str, count: usize) -> Box<Self> {
            let results = (0..count)
                .map(|_| {
                    Ok(ChatResponse {
                        message: ChatMessage::assistant("ok"),
                        usage: TokenUsage {
                            input_tokens: 1,
                            output_tokens: 1,
                        },
                        stop_reason: StopReason::EndTurn,
                    })
                })
                .collect();
            Box::new(Self {
                name: name.into(),
                results: StdMutex::new(results),
            })
        }

        fn failing(name: &str, count: usize) -> Box<Self> {
            Box::new(Self {
                name: name.into(),
                results: StdMutex::new(
                    (0..count)
                        .map(|_| Err(AivyxError::LlmProvider("mock failure".into())))
                        .collect(),
                ),
            })
        }

        fn with_sequence(name: &str, results: Vec<Result<ChatResponse>>) -> Box<Self> {
            Box::new(Self {
                name: name.into(),
                results: StdMutex::new(results),
            })
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
            let mut results = self.results.lock().unwrap();
            if results.is_empty() {
                Err(AivyxError::LlmProvider("no more mock results".into()))
            } else {
                results.remove(0)
            }
        }
    }

    fn test_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: std::time::Duration::from_millis(50),
            success_threshold: 1,
        }
    }

    fn dummy_request() -> ChatRequest {
        ChatRequest {
            system_prompt: None,
            messages: vec![ChatMessage::user("test")],
            tools: vec![],
            model: None,
            max_tokens: 10,
        }
    }

    #[tokio::test]
    async fn primary_succeeds_no_fallback_used() {
        let resilient = ResilientProvider::new(
            MockProvider::succeeding("primary", 3),
            "primary".into(),
            test_config(),
        )
        .with_fallback(
            MockProvider::succeeding("fallback", 3),
            "fallback".into(),
            test_config(),
        );

        let resp = resilient.chat(&dummy_request()).await.unwrap();
        assert_eq!(resp.message.content.to_text(), "ok");
        assert_eq!(resilient.name(), "primary");
    }

    #[tokio::test]
    async fn primary_fails_fallback_used() {
        // Primary fails immediately, fallback succeeds.
        let resilient = ResilientProvider::new(
            MockProvider::failing("primary", 1),
            "primary".into(),
            test_config(),
        )
        .with_fallback(
            MockProvider::succeeding("fallback", 1),
            "fallback".into(),
            test_config(),
        );

        let resp = resilient.chat(&dummy_request()).await.unwrap();
        assert_eq!(resp.message.content.to_text(), "ok");
    }

    #[tokio::test]
    async fn primary_circuit_opens_after_threshold() {
        let resilient = ResilientProvider::new(
            MockProvider::failing("primary", 5),
            "primary".into(),
            test_config(),
        )
        .with_fallback(
            MockProvider::succeeding("fallback", 5),
            "fallback".into(),
            test_config(),
        );

        // First call: primary fails (1/2), fallback succeeds.
        resilient.chat(&dummy_request()).await.unwrap();

        // Second call: primary fails (2/2 → circuit opens), fallback succeeds.
        resilient.chat(&dummy_request()).await.unwrap();

        // Third call: primary circuit is open (skipped), fallback succeeds.
        resilient.chat(&dummy_request()).await.unwrap();
        assert_eq!(resilient.name(), "fallback");
    }

    #[tokio::test]
    async fn failover_emits_observer_events() {
        let events: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(vec![]));
        let events_clone = events.clone();
        let observer: FailoverObserver = Arc::new(move |event| {
            let label = match &event {
                ProviderEvent::CircuitOpened { provider, .. } => {
                    format!("opened:{provider}")
                }
                ProviderEvent::CircuitClosed { provider } => {
                    format!("closed:{provider}")
                }
                ProviderEvent::FailoverActivated { from, to } => {
                    format!("failover:{from}->{to}")
                }
                ProviderEvent::AllProvidersDown => "all_down".into(),
            };
            events_clone.lock().unwrap().push(label);
        });

        let resilient = ResilientProvider::new(
            MockProvider::failing("primary", 5),
            "primary".into(),
            test_config(),
        )
        .with_fallback(
            MockProvider::succeeding("fallback", 5),
            "fallback".into(),
            test_config(),
        )
        .with_observer(observer);

        // Two calls to trip the primary circuit (threshold = 2).
        resilient.chat(&dummy_request()).await.unwrap();
        resilient.chat(&dummy_request()).await.unwrap();

        let captured = events.lock().unwrap();
        assert!(
            captured.iter().any(|e| e == "opened:primary"),
            "should have emitted CircuitOpened for primary, got: {captured:?}"
        );
    }

    #[tokio::test]
    async fn all_providers_down_returns_error() {
        let resilient = ResilientProvider::new(
            MockProvider::failing("primary", 1),
            "primary".into(),
            test_config(),
        )
        .with_fallback(
            MockProvider::failing("fallback", 1),
            "fallback".into(),
            test_config(),
        );

        let result = resilient.chat(&dummy_request()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn primary_recovers_after_timeout() {
        let ok_response = ChatResponse {
            message: ChatMessage::assistant("recovered"),
            usage: TokenUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
            stop_reason: StopReason::EndTurn,
        };

        // Primary: 2 failures then 1 success.
        let primary_results: Vec<Result<ChatResponse>> = vec![
            Err(AivyxError::LlmProvider("fail1".into())),
            Err(AivyxError::LlmProvider("fail2".into())),
            Ok(ok_response),
        ];

        let resilient = ResilientProvider::new(
            MockProvider::with_sequence("primary", primary_results),
            "primary".into(),
            test_config(),
        )
        .with_fallback(
            MockProvider::succeeding("fallback", 5),
            "fallback".into(),
            test_config(),
        );

        // Trip the primary (2 failures → circuit opens).
        resilient.chat(&dummy_request()).await.unwrap(); // primary fails, fallback ok
        resilient.chat(&dummy_request()).await.unwrap(); // primary fails (opens), fallback ok

        // Primary circuit is now open.
        assert_eq!(resilient.name(), "fallback");

        // Wait for recovery timeout.
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        // Next call: primary transitions to HalfOpen, probe succeeds → Closed.
        let resp = resilient.chat(&dummy_request()).await.unwrap();
        assert_eq!(resp.message.content.to_text(), "recovered");
        assert_eq!(resilient.name(), "primary");
    }
}
