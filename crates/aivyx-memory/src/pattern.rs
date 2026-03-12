//! Workflow pattern mining — detects repeated successful tool sequences
//! from outcome records and surfaces them as reusable skill suggestions.
//!
//! The mining process:
//! 1. Load successful outcomes with `tools_used` sequences
//! 2. Extract n-grams (2..=max_len) from each tool sequence
//! 3. Count frequency, success rate, and average duration per unique n-gram
//! 4. Filter by minimum occurrence and success thresholds
//! 5. Deduplicate subsequences (drop n-grams fully contained in a longer pattern)
//! 6. Persist as `WorkflowPattern` entries

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use aivyx_core::PatternId;

use crate::outcome::OutcomeRecord;

/// A discovered workflow pattern — a recurring tool sequence correlated with
/// successful outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPattern {
    /// Unique identifier.
    pub id: PatternId,
    /// The ordered tool sequence (e.g., `["shell", "file_read", "shell"]`).
    pub tool_sequence: Vec<String>,
    /// A canonical key for deduplication (tools joined with `→`).
    pub sequence_key: String,
    /// Keywords extracted from the goal contexts of matching outcomes.
    pub goal_keywords: Vec<String>,
    /// Success rate among outcomes containing this sequence (0.0–1.0).
    pub success_rate: f32,
    /// Total number of outcomes containing this sequence.
    pub occurrence_count: u32,
    /// Number of successful outcomes containing this sequence.
    pub success_count: u32,
    /// Average duration in milliseconds of successful outcomes.
    pub avg_duration_ms: u64,
    /// Agent roles that commonly execute this pattern.
    pub agent_roles: Vec<String>,
    /// Free-form tags.
    pub tags: Vec<String>,
    /// When this pattern was first discovered.
    pub created_at: DateTime<Utc>,
    /// When this pattern was last updated from new outcome data.
    pub updated_at: DateTime<Utc>,
}

impl WorkflowPattern {
    /// Create a sequence key from tools (e.g., `"shell→file_read→shell"`).
    pub fn make_key(tools: &[String]) -> String {
        tools.join("→")
    }
}

/// Configuration for the pattern mining process.
#[derive(Debug, Clone)]
pub struct MiningConfig {
    /// Minimum times a sequence must appear to be considered a pattern.
    pub min_occurrences: u32,
    /// Minimum success rate (0.0–1.0) for a pattern to be kept.
    pub min_success_rate: f32,
    /// Maximum n-gram length to extract.
    pub max_sequence_len: usize,
    /// Minimum n-gram length.
    pub min_sequence_len: usize,
    /// Maximum number of patterns to return.
    pub max_patterns: usize,
}

impl Default for MiningConfig {
    fn default() -> Self {
        Self {
            min_occurrences: 3,
            min_success_rate: 0.6,
            max_sequence_len: 6,
            min_sequence_len: 2,
            max_patterns: 50,
        }
    }
}

/// Filter criteria for querying stored patterns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatternFilter {
    /// Only patterns containing this tool.
    pub contains_tool: Option<String>,
    /// Minimum success rate.
    pub min_success_rate: Option<f32>,
    /// Minimum occurrence count.
    pub min_occurrences: Option<u32>,
    /// Maximum results.
    pub limit: Option<usize>,
}

/// Intermediate accumulator for an n-gram during mining.
#[derive(Debug)]
struct NgramStats {
    tools: Vec<String>,
    total: u32,
    successes: u32,
    total_duration_ms: u64,
    goal_words: HashMap<String, u32>,
    roles: HashMap<String, u32>,
}

/// Mine workflow patterns from a set of outcome records.
///
/// Returns newly discovered patterns sorted by occurrence count descending.
pub fn mine_patterns(outcomes: &[OutcomeRecord], config: &MiningConfig) -> Vec<WorkflowPattern> {
    // Phase 1: extract n-gram statistics
    let mut ngram_map: HashMap<String, NgramStats> = HashMap::new();

    for outcome in outcomes {
        if outcome.tools_used.len() < config.min_sequence_len {
            continue;
        }

        let tools = &outcome.tools_used;

        // Extract all n-grams of length min..=max
        for n in config.min_sequence_len..=config.max_sequence_len.min(tools.len()) {
            for window in tools.windows(n) {
                let key = window
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("→");

                let stats = ngram_map.entry(key).or_insert_with(|| NgramStats {
                    tools: window.iter().cloned().collect(),
                    total: 0,
                    successes: 0,
                    total_duration_ms: 0,
                    goal_words: HashMap::new(),
                    roles: HashMap::new(),
                });

                stats.total += 1;
                if outcome.success {
                    stats.successes += 1;
                    stats.total_duration_ms += outcome.duration_ms;
                }

                // Track goal keywords (simple word extraction)
                for word in extract_keywords(&outcome.goal_context) {
                    *stats.goal_words.entry(word).or_insert(0) += 1;
                }

                // Track agent roles
                if let Some(ref role) = outcome.agent_role {
                    *stats.roles.entry(role.clone()).or_insert(0) += 1;
                }
            }
        }
    }

    // Phase 2: filter by thresholds
    let mut candidates: Vec<(String, NgramStats)> = ngram_map
        .into_iter()
        .filter(|(_, s)| {
            s.total >= config.min_occurrences && {
                let rate = s.successes as f32 / s.total as f32;
                rate >= config.min_success_rate
            }
        })
        .collect();

    // Phase 3: deduplicate subsequences — remove n-grams that are strict
    // subsequences of a longer qualifying pattern
    let keys: Vec<String> = candidates.iter().map(|(k, _)| k.clone()).collect();
    candidates.retain(|(key, _)| {
        !keys.iter().any(|other| other.len() > key.len() && other.contains(key.as_str()))
    });

    // Phase 4: sort by occurrence count descending, take top N
    candidates.sort_by(|a, b| b.1.total.cmp(&a.1.total));
    candidates.truncate(config.max_patterns);

    // Phase 5: convert to WorkflowPattern
    let now = Utc::now();
    candidates
        .into_iter()
        .map(|(_, stats)| {
            let success_rate = stats.successes as f32 / stats.total as f32;
            let avg_duration = if stats.successes > 0 {
                stats.total_duration_ms / stats.successes as u64
            } else {
                0
            };

            // Top goal keywords (by frequency, max 10)
            let mut keywords: Vec<(String, u32)> = stats.goal_words.into_iter().collect();
            keywords.sort_by(|a, b| b.1.cmp(&a.1));
            let goal_keywords: Vec<String> =
                keywords.into_iter().take(10).map(|(w, _)| w).collect();

            // Top agent roles
            let mut roles: Vec<(String, u32)> = stats.roles.into_iter().collect();
            roles.sort_by(|a, b| b.1.cmp(&a.1));
            let agent_roles: Vec<String> = roles.into_iter().take(5).map(|(r, _)| r).collect();

            let sequence_key = WorkflowPattern::make_key(&stats.tools);

            WorkflowPattern {
                id: PatternId::new(),
                tool_sequence: stats.tools,
                sequence_key,
                goal_keywords,
                success_rate,
                occurrence_count: stats.total,
                success_count: stats.successes,
                avg_duration_ms: avg_duration,
                agent_roles,
                tags: vec!["auto-mined".into()],
                created_at: now,
                updated_at: now,
            }
        })
        .collect()
}

/// Generate a SKILL.md document from a workflow pattern.
///
/// The generated skill teaches an agent *when* and *how* to use the
/// discovered tool sequence, based on the pattern's goal keywords and
/// success statistics.
pub fn generate_skill_markdown(pattern: &WorkflowPattern) -> String {
    let tool_list = pattern
        .tool_sequence
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}. `{}`", i + 1, t))
        .collect::<Vec<_>>()
        .join("\n");

    let keywords = if pattern.goal_keywords.is_empty() {
        "general tasks".to_string()
    } else {
        pattern.goal_keywords.join(", ")
    };

    let roles = if pattern.agent_roles.is_empty() {
        "any agent".to_string()
    } else {
        pattern.agent_roles.join(", ")
    };

    format!(
        r#"# {name}

> Auto-discovered workflow pattern ({occurrences} occurrences, {rate:.0}% success rate)

## When to use

This pattern is effective for tasks involving: **{keywords}**

Best suited for roles: {roles}

## Tool sequence

{tool_list}

## Performance

- **Success rate**: {rate:.0}%
- **Avg duration**: {duration:.1}s
- **Observed**: {occurrences} times ({successes} successful)

## How to apply

Execute the tools in order. Each step builds on the output of the previous one.
Adapt the specific arguments based on the task at hand — the sequence itself is
what matters, not the exact parameters.
"#,
        name = pattern.sequence_key.replace('→', " → "),
        keywords = keywords,
        roles = roles,
        tool_list = tool_list,
        rate = pattern.success_rate * 100.0,
        duration = pattern.avg_duration_ms as f64 / 1000.0,
        occurrences = pattern.occurrence_count,
        successes = pattern.success_count,
    )
}

/// Extract simple keywords from a goal context string.
///
/// Lowercases, splits on whitespace/punctuation, and removes common
/// stop words and very short tokens.
fn extract_keywords(text: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has",
        "had", "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall",
        "can", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "about",
        "that", "this", "it", "its", "and", "or", "but", "not", "no", "if", "then", "than",
        "so", "up", "out", "all", "just", "also", "very", "too", "any", "each", "every",
    ];

    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(w))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::OutcomeId;
    use crate::outcome::OutcomeSource;

    fn make_outcome(
        tools: &[&str],
        success: bool,
        goal: &str,
        role: Option<&str>,
        duration_ms: u64,
    ) -> OutcomeRecord {
        OutcomeRecord {
            id: OutcomeId::new(),
            source: OutcomeSource::ToolCall {
                tool_name: tools.first().unwrap_or(&"unknown").to_string(),
            },
            success,
            result_summary: "ok".into(),
            duration_ms,
            agent_name: "test-agent".into(),
            agent_role: role.map(String::from),
            goal_context: goal.into(),
            tools_used: tools.iter().map(|s| s.to_string()).collect(),
            tags: vec![],
            created_at: Utc::now(),
            human_rating: None,
            human_feedback: None,
        }
    }

    #[test]
    fn mine_detects_recurring_sequence() {
        let outcomes: Vec<OutcomeRecord> = (0..5)
            .map(|_| {
                make_outcome(
                    &["shell", "file_read", "shell"],
                    true,
                    "debug build error",
                    Some("Coder"),
                    3000,
                )
            })
            .collect();

        let config = MiningConfig {
            min_occurrences: 3,
            ..Default::default()
        };
        let patterns = mine_patterns(&outcomes, &config);

        assert!(!patterns.is_empty(), "should find at least one pattern");

        // The full 3-gram should be present
        let full = patterns
            .iter()
            .find(|p| p.tool_sequence == ["shell", "file_read", "shell"]);
        assert!(full.is_some(), "should find the full 3-tool sequence");

        let p = full.unwrap();
        assert_eq!(p.occurrence_count, 5);
        assert!((p.success_rate - 1.0).abs() < f32::EPSILON);
        assert_eq!(p.avg_duration_ms, 3000);
    }

    #[test]
    fn mine_filters_by_success_rate() {
        let mut outcomes: Vec<OutcomeRecord> = (0..3)
            .map(|_| make_outcome(&["tool_a", "tool_b"], true, "task", None, 1000))
            .collect();
        // Add 7 failures — total success rate = 30%
        for _ in 0..7 {
            outcomes.push(make_outcome(
                &["tool_a", "tool_b"],
                false,
                "task",
                None,
                1000,
            ));
        }

        let config = MiningConfig {
            min_occurrences: 3,
            min_success_rate: 0.5, // 50% threshold
            ..Default::default()
        };
        let patterns = mine_patterns(&outcomes, &config);

        // 30% success rate < 50% threshold → no patterns
        assert!(
            patterns.is_empty(),
            "should filter out low success rate patterns"
        );
    }

    #[test]
    fn mine_deduplicates_subsequences() {
        // Create outcomes with a 4-tool sequence — this also generates 2-gram
        // and 3-gram subsequences, but the longer pattern should subsume them
        let outcomes: Vec<OutcomeRecord> = (0..5)
            .map(|_| {
                make_outcome(
                    &["plan", "shell", "file_read", "shell"],
                    true,
                    "implement feature",
                    None,
                    5000,
                )
            })
            .collect();

        let config = MiningConfig {
            min_occurrences: 3,
            ..Default::default()
        };
        let patterns = mine_patterns(&outcomes, &config);

        // The full 4-gram should be present
        let has_full = patterns
            .iter()
            .any(|p| p.tool_sequence == ["plan", "shell", "file_read", "shell"]);
        assert!(has_full, "should find the full 4-tool sequence");

        // Strict subsequences that are contained in the full key should be removed
        let has_sub = patterns
            .iter()
            .any(|p| p.sequence_key == "shell→file_read" || p.sequence_key == "file_read→shell");
        assert!(
            !has_sub,
            "should remove subsequences contained in longer patterns"
        );
    }

    #[test]
    fn mine_respects_min_occurrences() {
        let outcomes: Vec<OutcomeRecord> = (0..2)
            .map(|_| make_outcome(&["tool_x", "tool_y"], true, "rare task", None, 1000))
            .collect();

        let config = MiningConfig {
            min_occurrences: 3,
            ..Default::default()
        };
        let patterns = mine_patterns(&outcomes, &config);

        assert!(patterns.is_empty(), "2 occurrences < 3 minimum");
    }

    #[test]
    fn mine_extracts_goal_keywords() {
        let outcomes: Vec<OutcomeRecord> = (0..4)
            .map(|_| {
                make_outcome(
                    &["shell", "file_read"],
                    true,
                    "debug authentication error in login flow",
                    None,
                    2000,
                )
            })
            .collect();

        let config = MiningConfig {
            min_occurrences: 3,
            ..Default::default()
        };
        let patterns = mine_patterns(&outcomes, &config);

        assert!(!patterns.is_empty());
        let p = &patterns[0];
        assert!(
            p.goal_keywords.contains(&"debug".to_string())
                || p.goal_keywords.contains(&"authentication".to_string()),
            "should extract meaningful keywords: {:?}",
            p.goal_keywords
        );
    }

    #[test]
    fn mine_tracks_agent_roles() {
        let outcomes: Vec<OutcomeRecord> = (0..4)
            .map(|_| {
                make_outcome(
                    &["shell", "file_read"],
                    true,
                    "task",
                    Some("Debugger"),
                    1000,
                )
            })
            .collect();

        let config = MiningConfig {
            min_occurrences: 3,
            ..Default::default()
        };
        let patterns = mine_patterns(&outcomes, &config);

        assert!(!patterns.is_empty());
        assert!(patterns[0].agent_roles.contains(&"Debugger".to_string()));
    }

    #[test]
    fn mine_ignores_short_sequences() {
        let outcomes: Vec<OutcomeRecord> = (0..5)
            .map(|_| make_outcome(&["solo_tool"], true, "task", None, 1000))
            .collect();

        let config = MiningConfig {
            min_occurrences: 3,
            min_sequence_len: 2,
            ..Default::default()
        };
        let patterns = mine_patterns(&outcomes, &config);

        assert!(
            patterns.is_empty(),
            "single-tool outcomes should be skipped"
        );
    }

    #[test]
    fn generate_skill_produces_valid_markdown() {
        let pattern = WorkflowPattern {
            id: PatternId::new(),
            tool_sequence: vec!["shell".into(), "file_read".into(), "shell".into()],
            sequence_key: "shell→file_read→shell".into(),
            goal_keywords: vec!["debug".into(), "build".into(), "error".into()],
            success_rate: 0.85,
            occurrence_count: 12,
            success_count: 10,
            avg_duration_ms: 4500,
            agent_roles: vec!["Coder".into()],
            tags: vec!["auto-mined".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let md = generate_skill_markdown(&pattern);
        assert!(md.contains("shell → file_read → shell"));
        assert!(md.contains("85%"));
        assert!(md.contains("12 occurrences"));
        assert!(md.contains("debug, build, error"));
        assert!(md.contains("Coder"));
        assert!(md.contains("1. `shell`"));
        assert!(md.contains("2. `file_read`"));
        assert!(md.contains("3. `shell`"));
    }

    #[test]
    fn sequence_key_roundtrip() {
        let tools = vec!["a".into(), "b".into(), "c".into()];
        let key = WorkflowPattern::make_key(&tools);
        assert_eq!(key, "a→b→c");
    }

    #[test]
    fn extract_keywords_filters_stop_words() {
        let words = extract_keywords("Fix the build error in the authentication module");
        assert!(words.contains(&"fix".to_string()));
        assert!(words.contains(&"build".to_string()));
        assert!(words.contains(&"error".to_string()));
        assert!(words.contains(&"authentication".to_string()));
        assert!(!words.contains(&"the".to_string()));
        assert!(!words.contains(&"in".to_string()));
    }

    #[test]
    fn mining_config_defaults() {
        let config = MiningConfig::default();
        assert_eq!(config.min_occurrences, 3);
        assert!((config.min_success_rate - 0.6).abs() < f32::EPSILON);
        assert_eq!(config.max_sequence_len, 6);
        assert_eq!(config.min_sequence_len, 2);
        assert_eq!(config.max_patterns, 50);
    }

    #[test]
    fn pattern_filter_defaults() {
        let filter = PatternFilter::default();
        assert!(filter.contains_tool.is_none());
        assert!(filter.min_success_rate.is_none());
        assert!(filter.min_occurrences.is_none());
        assert!(filter.limit.is_none());
    }
}
