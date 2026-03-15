use std::collections::HashMap;

use aivyx_core::{AivyxError, Result};
use aivyx_llm::TokenUsage;

/// Per-category cost breakdown entry.
#[derive(Debug, Clone, Default)]
pub struct CostEntry {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub call_count: u64,
}

/// Tracks estimated cost from LLM token usage across a session.
///
/// Enforces a configurable per-session spending cap. The `metadata` map
/// can carry observer-populated tags (e.g., complexity level from routing)
/// that the engine-side cost ledger reads when recording entries.
///
/// Per-category breakdowns (`by_category`) attribute costs to logical
/// operations like "turn", "compression", "extraction", or specific tool
/// names — enabling granular cost analysis.
pub struct CostTracker {
    max_cost_usd: f64,
    accumulated_cost_usd: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    input_cost_per_token: f64,
    output_cost_per_token: f64,
    /// Metadata populated by observers (e.g., routing complexity level).
    pub metadata: HashMap<String, String>,
    /// Per-category cost breakdown (e.g., "turn", "compression", "extraction", tool names).
    by_category: HashMap<String, CostEntry>,
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
            metadata: HashMap::new(),
            by_category: HashMap::new(),
        }
    }

    /// Record token usage and check if the session cost cap is exceeded.
    pub fn track(&mut self, usage: &TokenUsage) -> Result<()> {
        self.track_with_category(usage, "turn")
    }

    /// Record token usage attributed to a named category.
    ///
    /// Categories are free-form strings like `"turn"`, `"compression"`,
    /// `"extraction"`, or tool names. The per-category breakdown can be
    /// queried via [`cost_by_category()`](Self::cost_by_category).
    pub fn track_with_category(&mut self, usage: &TokenUsage, category: &str) -> Result<()> {
        let cost = (usage.input_tokens as f64 * self.input_cost_per_token)
            + (usage.output_tokens as f64 * self.output_cost_per_token);

        self.accumulated_cost_usd += cost;
        self.total_input_tokens += usage.input_tokens as u64;
        self.total_output_tokens += usage.output_tokens as u64;

        let entry = self.by_category.entry(category.to_string()).or_default();
        entry.input_tokens += usage.input_tokens as u64;
        entry.output_tokens += usage.output_tokens as u64;
        entry.cost_usd += cost;
        entry.call_count += 1;

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

    /// Per-category cost breakdown.
    pub fn cost_by_category(&self) -> &HashMap<String, CostEntry> {
        &self.by_category
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

    #[test]
    fn track_with_category_attributes_costs() {
        let mut tracker = CostTracker::new(100.0, 0.000003, 0.000015);

        tracker
            .track_with_category(
                &TokenUsage {
                    input_tokens: 1000,
                    output_tokens: 500,
                },
                "turn",
            )
            .unwrap();

        tracker
            .track_with_category(
                &TokenUsage {
                    input_tokens: 200,
                    output_tokens: 100,
                },
                "compression",
            )
            .unwrap();

        tracker
            .track_with_category(
                &TokenUsage {
                    input_tokens: 300,
                    output_tokens: 150,
                },
                "extraction",
            )
            .unwrap();

        // Second turn call to test aggregation within a category
        tracker
            .track_with_category(
                &TokenUsage {
                    input_tokens: 500,
                    output_tokens: 250,
                },
                "turn",
            )
            .unwrap();

        let cats = tracker.cost_by_category();
        assert_eq!(cats.len(), 3);

        let turn = &cats["turn"];
        assert_eq!(turn.input_tokens, 1500);
        assert_eq!(turn.output_tokens, 750);
        assert_eq!(turn.call_count, 2);
        assert!(turn.cost_usd > 0.0);

        let compression = &cats["compression"];
        assert_eq!(compression.input_tokens, 200);
        assert_eq!(compression.call_count, 1);

        let extraction = &cats["extraction"];
        assert_eq!(extraction.input_tokens, 300);
        assert_eq!(extraction.call_count, 1);

        // Total should equal sum of categories
        assert_eq!(tracker.total_input_tokens(), 2000);
        assert_eq!(tracker.total_output_tokens(), 1000);
    }

    #[test]
    fn track_defaults_to_turn_category() {
        let mut tracker = CostTracker::new(100.0, 0.000003, 0.000015);
        tracker
            .track(&TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
            })
            .unwrap();

        let cats = tracker.cost_by_category();
        assert_eq!(cats.len(), 1);
        assert!(cats.contains_key("turn"));
        assert_eq!(cats["turn"].call_count, 1);
    }
}
