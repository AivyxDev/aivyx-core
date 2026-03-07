//! Outcome tracking for agent self-improvement.
//!
//! Records the success/failure of mission steps, tool calls, and team
//! delegations. This data feeds the planner feedback loop, specialist
//! recommendation learning, and skill effectiveness scoring.

use aivyx_core::{OutcomeId, TaskId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}
