//! Outcome tracking for agent self-improvement.
//!
//! Records the success/failure of mission steps, tool calls, and team
//! delegations. This data feeds the planner feedback loop, specialist
//! recommendation learning, and skill effectiveness scoring.

use aivyx_core::{OutcomeId, TaskId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::notification::Rating;

/// A recorded outcome from an agent operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeRecord {
    /// Unique identifier.
    pub id: OutcomeId,
    /// What produced this outcome.
    pub source: OutcomeSource,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Summary of the result (truncated to 500 chars).
    pub result_summary: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Agent that executed this.
    pub agent_name: String,
    /// Agent's role (for team contexts).
    pub agent_role: Option<String>,
    /// Goal or task description that was being pursued.
    pub goal_context: String,
    /// Tool(s) used during execution.
    pub tools_used: Vec<String>,
    /// Tags for categorization.
    pub tags: Vec<String>,
    /// When this outcome was recorded.
    pub created_at: DateTime<Utc>,
    /// Human feedback rating (None = unrated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_rating: Option<Rating>,
    /// Human feedback comment (None = no feedback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_feedback: Option<String>,
}

/// What produced an outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutcomeSource {
    /// A mission step (task_id + step_index).
    MissionStep { task_id: TaskId, step_index: usize },
    /// A tool invocation.
    ToolCall { tool_name: String },
    /// A team delegation.
    Delegation { specialist: String, task: String },
    /// A specialist suggestion that was followed.
    SpecialistSuggestion { suggested: String, chosen: String },
}

impl OutcomeRecord {
    /// Create a new outcome record with the current timestamp.
    pub fn new(
        source: OutcomeSource,
        success: bool,
        result_summary: String,
        duration_ms: u64,
        agent_name: String,
        goal_context: String,
    ) -> Self {
        // Truncate result summary to 500 chars.
        let summary = if result_summary.len() > 500 {
            format!("{}...", &result_summary[..497])
        } else {
            result_summary
        };

        Self {
            id: OutcomeId::new(),
            source,
            success,
            result_summary: summary,
            duration_ms,
            agent_name,
            agent_role: None,
            goal_context,
            tools_used: Vec::new(),
            tags: Vec::new(),
            created_at: Utc::now(),
            human_rating: None,
            human_feedback: None,
        }
    }

    /// Set the agent role.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.agent_role = Some(role.into());
        self
    }

    /// Set the tools used.
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools_used = tools;
        self
    }

    /// Set tags.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Set a human rating on this outcome.
    pub fn with_rating(mut self, rating: Rating) -> Self {
        self.human_rating = Some(rating);
        self
    }

    /// Set human feedback text on this outcome.
    pub fn with_feedback(mut self, feedback: impl Into<String>) -> Self {
        self.human_feedback = Some(feedback.into());
        self
    }

    /// Whether this outcome has been rated by a human.
    pub fn is_rated(&self) -> bool {
        self.human_rating.is_some()
    }
}

/// Filter criteria for querying outcomes.
#[derive(Debug, Default)]
pub struct OutcomeFilter {
    /// Filter by source type name (e.g., "MissionStep", "Delegation").
    pub source_type: Option<String>,
    /// Filter by success/failure.
    pub success: Option<bool>,
    /// Filter by agent name.
    pub agent_name: Option<String>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Filter by human-rated status: `Some(true)` = rated only, `Some(false)` = unrated only.
    pub rated: Option<bool>,
    /// Filter by specific human rating value.
    pub rating: Option<Rating>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_record_new() {
        let record = OutcomeRecord::new(
            OutcomeSource::MissionStep {
                task_id: TaskId::new(),
                step_index: 0,
            },
            true,
            "Step completed successfully".into(),
            1500,
            "test-agent".into(),
            "deploy the API".into(),
        );
        assert!(record.success);
        assert_eq!(record.agent_name, "test-agent");
        assert!(record.agent_role.is_none());
        assert!(record.tools_used.is_empty());
    }

    #[test]
    fn outcome_record_with_builder() {
        let record = OutcomeRecord::new(
            OutcomeSource::Delegation {
                specialist: "coder".into(),
                task: "write tests".into(),
            },
            false,
            "Failed due to timeout".into(),
            30000,
            "lead".into(),
            "implement feature".into(),
        )
        .with_role("coordinator")
        .with_tools(vec!["shell".into(), "file_write".into()])
        .with_tags(vec!["team".into()]);

        assert!(!record.success);
        assert_eq!(record.agent_role.as_deref(), Some("coordinator"));
        assert_eq!(record.tools_used.len(), 2);
        assert_eq!(record.tags, vec!["team"]);
    }

    #[test]
    fn outcome_record_truncates_summary() {
        let long_summary = "x".repeat(600);
        let record = OutcomeRecord::new(
            OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            true,
            long_summary,
            100,
            "agent".into(),
            "goal".into(),
        );
        assert_eq!(record.result_summary.len(), 500);
        assert!(record.result_summary.ends_with("..."));
    }

    #[test]
    fn outcome_record_serde_roundtrip() {
        let record = OutcomeRecord::new(
            OutcomeSource::MissionStep {
                task_id: TaskId::new(),
                step_index: 3,
            },
            true,
            "done".into(),
            500,
            "agent".into(),
            "goal".into(),
        )
        .with_role("coder")
        .with_tools(vec!["file_write".into()]);

        let json = serde_json::to_string(&record).unwrap();
        let restored: OutcomeRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, record.id);
        assert_eq!(restored.success, true);
        assert_eq!(restored.agent_role.as_deref(), Some("coder"));
        assert_eq!(restored.tools_used, vec!["file_write"]);
    }

    #[test]
    fn outcome_source_variants_serde() {
        let sources = vec![
            OutcomeSource::MissionStep {
                task_id: TaskId::new(),
                step_index: 0,
            },
            OutcomeSource::ToolCall {
                tool_name: "shell".into(),
            },
            OutcomeSource::Delegation {
                specialist: "coder".into(),
                task: "write code".into(),
            },
            OutcomeSource::SpecialistSuggestion {
                suggested: "coder".into(),
                chosen: "researcher".into(),
            },
        ];

        for source in &sources {
            let json = serde_json::to_string(source).unwrap();
            let restored: OutcomeSource = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&restored).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn outcome_record_new_has_no_rating() {
        let record = OutcomeRecord::new(
            OutcomeSource::ToolCall { tool_name: "shell".into() },
            true,
            "ok".into(),
            100,
            "agent".into(),
            "goal".into(),
        );
        assert!(!record.is_rated());
        assert!(record.human_rating.is_none());
        assert!(record.human_feedback.is_none());
    }

    #[test]
    fn outcome_record_with_rating_and_feedback() {
        let record = OutcomeRecord::new(
            OutcomeSource::ToolCall { tool_name: "shell".into() },
            true,
            "ok".into(),
            100,
            "agent".into(),
            "goal".into(),
        )
        .with_rating(Rating::Useful)
        .with_feedback("Great result!");

        assert!(record.is_rated());
        assert_eq!(record.human_rating, Some(Rating::Useful));
        assert_eq!(record.human_feedback.as_deref(), Some("Great result!"));
    }

    #[test]
    fn outcome_record_serde_with_rating() {
        let record = OutcomeRecord::new(
            OutcomeSource::ToolCall { tool_name: "shell".into() },
            true,
            "ok".into(),
            100,
            "agent".into(),
            "goal".into(),
        )
        .with_rating(Rating::Partial)
        .with_feedback("Needs improvement");

        let json = serde_json::to_string(&record).unwrap();
        let restored: OutcomeRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.human_rating, Some(Rating::Partial));
        assert_eq!(restored.human_feedback.as_deref(), Some("Needs improvement"));
    }

    #[test]
    fn outcome_record_serde_backward_compat() {
        // Simulate a record serialized without the new fields
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "source": {"ToolCall": {"tool_name": "shell"}},
            "success": true,
            "result_summary": "ok",
            "duration_ms": 100,
            "agent_name": "agent",
            "goal_context": "goal",
            "tools_used": [],
            "tags": [],
            "created_at": "2025-01-01T00:00:00Z"
        }"#;
        let record: OutcomeRecord = serde_json::from_str(json).unwrap();
        assert!(record.human_rating.is_none());
        assert!(record.human_feedback.is_none());
        assert!(!record.is_rated());
    }
}
