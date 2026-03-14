use aivyx_core::{AivyxError, Result};
use aivyx_llm::TokenUsage;

/// Tracks estimated cost from LLM token usage across a session.
///
/// Enforces a configurable per-session spending cap.
pub struct CostTracker {
    max_cost_usd: f64,
    accumulated_cost_usd: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    input_cost_per_token: f64,
    output_cost_per_token: f64,
}

impl CostTracker {
    pub fn new(max_cost_usd: f64, input_cost_per_token: f64, output_cost_per_token: f64) -> Self {
        Self {
            max_cost_usd,
            accumulated_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            input_cost_per_token,
            output_cost_per_token,
        }
    }

    /// Record token usage and check if the session cost cap is exceeded.
    pub fn track(&mut self, usage: &TokenUsage) -> Result<()> {
        let cost = (usage.input_tokens as f64 * self.input_cost_per_token)
            + (usage.output_tokens as f64 * self.output_cost_per_token);

        self.accumulated_cost_usd += cost;
        self.total_input_tokens += usage.input_tokens as u64;
        self.total_output_tokens += usage.output_tokens as u64;

        // max_cost_usd == 0.0 means "unlimited" (no cap enforcement).
        // This is the natural default for local providers like Ollama.
        if self.max_cost_usd > 0.0 && self.accumulated_cost_usd > self.max_cost_usd {
            return Err(AivyxError::Agent(format!(
                "session cost cap exceeded: ${:.4} > ${:.2}",
                self.accumulated_cost_usd, self.max_cost_usd
            )));
        }

        Ok(())
    }

    /// Current accumulated cost in USD.
    pub fn current_cost_usd(&self) -> f64 {
        self.accumulated_cost_usd
    }

    /// Total input tokens used this session.
    pub fn total_input_tokens(&self) -> u64 {
        self.total_input_tokens
    }

    /// Total output tokens used this session.
    pub fn total_output_tokens(&self) -> u64 {
        self.total_output_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_cost() {
        let mut tracker = CostTracker::new(10.0, 0.000003, 0.000015);
        tracker
            .track(&TokenUsage {
                input_tokens: 1000,
                output_tokens: 500,
            })
            .unwrap();

        assert!(tracker.current_cost_usd() > 0.0);
        assert_eq!(tracker.total_input_tokens(), 1000);
        assert_eq!(tracker.total_output_tokens(), 500);
    }

    #[test]
    fn enforces_cap() {
        let mut tracker = CostTracker::new(0.001, 0.000003, 0.000015);
        let result = tracker.track(&TokenUsage {
            input_tokens: 100_000,
            output_tokens: 100_000,
        });
        assert!(result.is_err());
    }

    #[test]
    fn accumulates_across_calls() {
        let mut tracker = CostTracker::new(100.0, 0.000003, 0.000015);
        tracker
            .track(&TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
            })
            .unwrap();
        let cost1 = tracker.current_cost_usd();

        tracker
            .track(&TokenUsage {
                input_tokens: 200,
                output_tokens: 100,
            })
            .unwrap();
        let cost2 = tracker.current_cost_usd();

        assert!(cost2 > cost1);
        assert_eq!(tracker.total_input_tokens(), 300);
    }

    #[test]
    fn custom_rates_compute_correctly() {
        // Opus rates: $15/$75 per 1M tokens
        let mut tracker = CostTracker::new(100.0, 0.000015, 0.000075);
        tracker
            .track(&TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            })
            .unwrap();
        // Expected: $15 + $75 = $90
        let cost = tracker.current_cost_usd();
        assert!((cost - 90.0).abs() < 0.01);
    }

    #[test]
    fn zero_rates_for_local_models() {
        let mut tracker = CostTracker::new(100.0, 0.0, 0.0);
        tracker
            .track(&TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            })
            .unwrap();
        assert!((tracker.current_cost_usd() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_cap_means_unlimited() {
        // max_cost_usd = 0.0 should mean "no limit", not "zero budget".
        let mut tracker = CostTracker::new(0.0, 0.000015, 0.000075);
        // Simulate heavy usage that would exceed any finite cap.
        tracker
            .track(&TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            })
            .unwrap();
        assert!(tracker.current_cost_usd() > 0.0);
        // Should still succeed — no cap enforcement.
        tracker
            .track(&TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            })
            .unwrap();
    }
}
