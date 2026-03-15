//! Circuit breaker for LLM provider resilience.
//!
//! Tracks consecutive failures for a single provider and transitions through
//! three states to prevent cascading failures:
//!
//! ```text
//! Closed ──[failures ≥ threshold]──→ Open
//!   ↑                                  │
//!   │                        [recovery timeout]
//!   │                                  ↓
//!   └──────[success]────────── HalfOpen
//!                                │
//!                          [failure]
//!                                ↓
//!                              Open (reset timer)
//! ```
//!
//! Uses `std::sync::Mutex` (not tokio) because state transitions are
//! sub-microsecond operations that must not be held across await points.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Provider considered down — requests fail fast without calling provider.
    Open,
    /// Testing recovery — one probe request allowed through.
    HalfOpen,
}

/// Configuration for a circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// How long the circuit stays open before transitioning to half-open.
    pub recovery_timeout: Duration,
    /// Number of consecutive successes in half-open needed to close circuit.
    pub success_threshold: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(30),
            success_threshold: 1,
        }
    }
}

/// Per-provider circuit breaker.
///
/// Thread-safe via interior mutability (`std::sync::Mutex`). All methods
/// are synchronous — call them before/after async provider calls, not
/// while holding the lock across an await.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    inner: Mutex<Inner>,
}

struct Inner {
    state: CircuitState,
    consecutive_failures: u32,
    consecutive_successes: u32,
    last_failure_at: Option<Instant>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker in the [`Closed`](CircuitState::Closed) state.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            inner: Mutex::new(Inner {
                state: CircuitState::Closed,
                consecutive_failures: 0,
                consecutive_successes: 0,
                last_failure_at: None,
            }),
        }
    }

    /// Check whether a request should be allowed through.
    ///
    /// - **Closed:** always returns `true`.
    /// - **Open:** returns `false` unless the recovery timeout has elapsed,
    ///   in which case the circuit transitions to **HalfOpen** and returns `true`.
    /// - **HalfOpen:** returns `true` (allow the probe request).
    pub fn can_execute(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if recovery timeout has elapsed.
                if let Some(last_failure) = inner.last_failure_at {
                    if last_failure.elapsed() >= self.config.recovery_timeout {
                        inner.state = CircuitState::HalfOpen;
                        inner.consecutive_successes = 0;
                        return true;
                    }
                }
                false
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful request.
    ///
    /// - **Closed:** resets failure counter.
    /// - **HalfOpen:** increments success counter; closes circuit if threshold met.
    /// - **Open:** no effect (shouldn't be called in this state).
    pub fn record_success(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.consecutive_failures = 0;
        match inner.state {
            CircuitState::Closed => {}
            CircuitState::HalfOpen => {
                inner.consecutive_successes += 1;
                if inner.consecutive_successes >= self.config.success_threshold {
                    inner.state = CircuitState::Closed;
                    inner.consecutive_successes = 0;
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Record a failed request.
    ///
    /// - **Closed:** increments failure counter; opens circuit if threshold met.
    /// - **HalfOpen:** immediately reopens circuit.
    /// - **Open:** no effect (shouldn't be called in this state).
    ///
    /// Returns `true` if the circuit just transitioned to [`Open`](CircuitState::Open).
    pub fn record_failure(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.last_failure_at = Some(Instant::now());
        inner.consecutive_successes = 0;
        match inner.state {
            CircuitState::Closed => {
                inner.consecutive_failures += 1;
                if inner.consecutive_failures >= self.config.failure_threshold {
                    inner.state = CircuitState::Open;
                    return true;
                }
                false
            }
            CircuitState::HalfOpen => {
                inner.state = CircuitState::Open;
                inner.consecutive_failures = 1;
                true
            }
            CircuitState::Open => false,
        }
    }

    /// Current circuit state.
    pub fn state(&self) -> CircuitState {
        self.inner.lock().unwrap().state
    }

    /// Current consecutive failure count.
    pub fn failure_count(&self) -> u32 {
        self.inner.lock().unwrap().consecutive_failures
    }

    /// Force the circuit back to [`Closed`](CircuitState::Closed).
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.state = CircuitState::Closed;
        inner.consecutive_failures = 0;
        inner.consecutive_successes = 0;
        inner.last_failure_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_millis(50),
            success_threshold: 1,
        }
    }

    #[test]
    fn new_starts_closed() {
        let cb = CircuitBreaker::new(default_config());
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.can_execute());
    }

    #[test]
    fn stays_closed_on_success() {
        let cb = CircuitBreaker::new(default_config());
        cb.record_success();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn opens_after_threshold_failures() {
        let cb = CircuitBreaker::new(default_config());

        assert!(!cb.record_failure()); // 1/3
        assert_eq!(cb.state(), CircuitState::Closed);

        assert!(!cb.record_failure()); // 2/3
        assert_eq!(cb.state(), CircuitState::Closed);

        assert!(cb.record_failure()); // 3/3 → Open
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn open_rejects_requests() {
        let cb = CircuitBreaker::new(default_config());

        // Trip the breaker.
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());
    }

    #[test]
    fn transitions_to_half_open_after_timeout() {
        let cb = CircuitBreaker::new(default_config());

        // Trip the breaker.
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for recovery timeout.
        std::thread::sleep(Duration::from_millis(60));

        // Should transition to HalfOpen on next can_execute().
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn half_open_closes_on_success() {
        let cb = CircuitBreaker::new(default_config());

        // Trip → wait → HalfOpen.
        for _ in 0..3 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(60));
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Successful probe → Closed.
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn half_open_reopens_on_failure() {
        let cb = CircuitBreaker::new(default_config());

        // Trip → wait → HalfOpen.
        for _ in 0..3 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(60));
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Failed probe → back to Open.
        assert!(cb.record_failure());
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn reset_forces_closed() {
        let cb = CircuitBreaker::new(default_config());

        // Trip the breaker.
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);

        // Force reset.
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 0);
        assert!(cb.can_execute());
    }
}
