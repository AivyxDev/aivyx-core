//! The `Task` struct — a persistent object tracking a multi-step goal.

use aivyx_core::{SessionId, TaskId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::status::TaskStatus;
use crate::step::TaskStep;

/// Priority level for a task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskPriority::Low => write!(f, "low"),
            TaskPriority::Normal => write!(f, "normal"),
            TaskPriority::High => write!(f, "high"),
            TaskPriority::Critical => write!(f, "critical"),
        }
    }
}

/// A persistent task tracking a user goal through planning, execution, and completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique identifier.
    pub id: TaskId,
    /// The user's goal or instruction.
    pub goal: String,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// Decomposed steps (populated after planning).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<TaskStep>,
    /// Index of the step currently being executed.
    pub current_step: usize,
    /// Name of the agent executing this task.
    pub agent_name: String,
    /// Session in which this task was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// When the task was created.
    pub created_at: DateTime<Utc>,
    /// When execution started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the task reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Final output produced by the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Error message if the task failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Task priority.
    #[serde(default)]
    pub priority: TaskPriority,
    /// Freeform tags for categorisation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Parent task ID for sub-task hierarchies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<TaskId>,
}

impl Task {
    /// Create a new task in `Planning` status.
    pub fn new(goal: impl Into<String>, agent_name: impl Into<String>) -> Self {
        Self {
            id: TaskId::new(),
            goal: goal.into(),
            status: TaskStatus::Planning,
            steps: Vec::new(),
            current_step: 0,
            agent_name: agent_name.into(),
            session_id: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            output: None,
            error: None,
            priority: TaskPriority::default(),
            tags: Vec::new(),
            parent_task_id: None,
        }
    }

    /// Set the decomposed steps, transitioning to `Planned`.
    pub fn set_steps(&mut self, steps: Vec<TaskStep>) {
        self.steps = steps;
        self.status = TaskStatus::Planned;
    }

    /// Begin execution, transitioning to `Executing`.
    pub fn start(&mut self) {
        self.status = TaskStatus::Executing;
        self.started_at = Some(Utc::now());
    }

    /// Advance to the next step. Returns `true` if there are more steps.
    pub fn next_step(&mut self) -> bool {
        if self.current_step + 1 < self.steps.len() {
            self.current_step += 1;
            true
        } else {
            false
        }
    }

    /// Count of steps with `Completed` status.
    pub fn completed_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.status == crate::step::StepStatus::Completed)
            .count()
    }

    /// Mark the task as completed with an optional output.
    pub fn complete(&mut self, output: Option<String>) {
        self.status = TaskStatus::Completed;
        self.completed_at = Some(Utc::now());
        self.output = output;
    }

    /// Mark the task as failed with an error message.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = TaskStatus::Failed;
        self.completed_at = Some(Utc::now());
        self.error = Some(error.into());
    }

    /// Cancel the task.
    pub fn cancel(&mut self) {
        self.status = TaskStatus::Cancelled;
        self.completed_at = Some(Utc::now());
    }

    /// Wall-clock duration from `started_at` to `completed_at` in milliseconds.
    ///
    /// Returns `None` if the task hasn't started or hasn't reached a terminal state.
    /// For an in-progress task, use [`elapsed_ms`](Self::elapsed_ms) instead.
    pub fn duration_ms(&self) -> Option<u64> {
        let started = self.started_at?;
        let completed = self.completed_at?;
        let duration = completed.signed_duration_since(started);
        Some(duration.num_milliseconds().max(0) as u64)
    }

    /// Wall-clock time since execution started, in milliseconds.
    ///
    /// Returns `None` if the task hasn't started. For completed tasks,
    /// this returns the same value as [`duration_ms`](Self::duration_ms).
    pub fn elapsed_ms(&self) -> Option<u64> {
        let started = self.started_at?;
        let end = self.completed_at.unwrap_or_else(Utc::now);
        let duration = end.signed_duration_since(started);
        Some(duration.num_milliseconds().max(0) as u64)
    }

    /// Sum of all individual step durations in milliseconds.
    ///
    /// Only includes steps that have recorded a `duration_ms`. Useful for
    /// measuring actual agent work time (excluding idle/approval waits).
    pub fn total_step_duration_ms(&self) -> u64 {
        self.steps.iter().filter_map(|s| s.duration_ms).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step::{StepStatus, TaskStep};

    #[test]
    fn new_creates_planning_task() {
        let task = Task::new("Research Rust async patterns", "assistant");
        assert_eq!(task.goal, "Research Rust async patterns");
        assert_eq!(task.agent_name, "assistant");
        assert_eq!(task.status, TaskStatus::Planning);
        assert!(task.steps.is_empty());
        assert_eq!(task.current_step, 0);
        assert!(task.started_at.is_none());
        assert!(task.completed_at.is_none());
        assert!(task.output.is_none());
        assert!(task.error.is_none());
        assert_eq!(task.priority, TaskPriority::Normal);
        assert!(task.tags.is_empty());
        assert!(task.parent_task_id.is_none());
    }

    #[test]
    fn set_steps_transitions_to_planned() {
        let mut task = Task::new("goal", "agent");
        let steps = vec![TaskStep::new(0, "step one"), TaskStep::new(1, "step two")];
        task.set_steps(steps);
        assert_eq!(task.status, TaskStatus::Planned);
        assert_eq!(task.steps.len(), 2);
    }

    #[test]
    fn start_transitions_to_executing() {
        let mut task = Task::new("goal", "agent");
        task.start();
        assert_eq!(task.status, TaskStatus::Executing);
        assert!(task.started_at.is_some());
    }

    #[test]
    fn next_step_advances() {
        let mut task = Task::new("goal", "agent");
        task.set_steps(vec![
            TaskStep::new(0, "a"),
            TaskStep::new(1, "b"),
            TaskStep::new(2, "c"),
        ]);
        assert_eq!(task.current_step, 0);
        assert!(task.next_step());
        assert_eq!(task.current_step, 1);
        assert!(task.next_step());
        assert_eq!(task.current_step, 2);
        assert!(!task.next_step());
        assert_eq!(task.current_step, 2);
    }

    #[test]
    fn completed_steps_counts_correctly() {
        let mut task = Task::new("goal", "agent");
        let mut steps = vec![
            TaskStep::new(0, "a"),
            TaskStep::new(1, "b"),
            TaskStep::new(2, "c"),
        ];
        steps[0].status = StepStatus::Completed;
        steps[2].status = StepStatus::Completed;
        task.steps = steps;
        assert_eq!(task.completed_steps(), 2);
    }

    #[test]
    fn complete_sets_terminal_state() {
        let mut task = Task::new("goal", "agent");
        task.complete(Some("result".into()));
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(task.completed_at.is_some());
        assert_eq!(task.output.as_deref(), Some("result"));
    }

    #[test]
    fn fail_sets_error() {
        let mut task = Task::new("goal", "agent");
        task.fail("something broke");
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(task.completed_at.is_some());
        assert_eq!(task.error.as_deref(), Some("something broke"));
    }

    #[test]
    fn cancel_sets_cancelled() {
        let mut task = Task::new("goal", "agent");
        task.cancel();
        assert_eq!(task.status, TaskStatus::Cancelled);
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn serde_roundtrip() {
        let mut task = Task::new("research", "agent-1");
        task.priority = TaskPriority::High;
        task.tags = vec!["research".into(), "urgent".into()];
        task.set_steps(vec![TaskStep::new(0, "step 1")]);

        let json = serde_json::to_string(&task).unwrap();
        let parsed: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, task.id);
        assert_eq!(parsed.goal, "research");
        assert_eq!(parsed.status, TaskStatus::Planned);
        assert_eq!(parsed.priority, TaskPriority::High);
        assert_eq!(parsed.tags, vec!["research", "urgent"]);
        assert_eq!(parsed.steps.len(), 1);
    }

    #[test]
    fn subtask_hierarchy() {
        let parent = Task::new("parent goal", "agent");
        let parent_id = parent.id;
        let mut child = Task::new("child goal", "agent");
        child.parent_task_id = Some(parent_id);
        assert_eq!(child.parent_task_id, Some(parent_id));
    }

    #[test]
    fn priority_display() {
        assert_eq!(TaskPriority::Low.to_string(), "low");
        assert_eq!(TaskPriority::Normal.to_string(), "normal");
        assert_eq!(TaskPriority::High.to_string(), "high");
        assert_eq!(TaskPriority::Critical.to_string(), "critical");
    }

    #[test]
    fn duration_ms_completed_task() {
        let mut task = Task::new("goal", "agent");
        task.started_at = Some(Utc::now() - chrono::Duration::seconds(5));
        task.completed_at = Some(Utc::now());
        let dur = task.duration_ms().unwrap();
        assert!(dur >= 4900 && dur <= 5200); // ~5000ms with tolerance
    }

    #[test]
    fn duration_ms_none_when_not_started() {
        let task = Task::new("goal", "agent");
        assert!(task.duration_ms().is_none());
    }

    #[test]
    fn duration_ms_none_when_still_running() {
        let mut task = Task::new("goal", "agent");
        task.start();
        assert!(task.duration_ms().is_none());
    }

    #[test]
    fn elapsed_ms_running_task() {
        let mut task = Task::new("goal", "agent");
        task.started_at = Some(Utc::now() - chrono::Duration::milliseconds(100));
        let elapsed = task.elapsed_ms().unwrap();
        assert!(elapsed >= 90); // at least ~100ms
    }

    #[test]
    fn elapsed_ms_completed_equals_duration() {
        let mut task = Task::new("goal", "agent");
        task.started_at = Some(Utc::now() - chrono::Duration::seconds(3));
        task.completed_at = Some(Utc::now());
        assert_eq!(task.elapsed_ms(), task.duration_ms());
    }

    #[test]
    fn total_step_duration_ms_sums_recorded() {
        let mut task = Task::new("goal", "agent");
        let mut steps = vec![
            TaskStep::new(0, "a"),
            TaskStep::new(1, "b"),
            TaskStep::new(2, "c"),
        ];
        steps[0].duration_ms = Some(1000);
        steps[1].duration_ms = None; // not yet recorded
        steps[2].duration_ms = Some(2500);
        task.steps = steps;
        assert_eq!(task.total_step_duration_ms(), 3500);
    }

    #[test]
    fn total_step_duration_ms_empty_steps() {
        let task = Task::new("goal", "agent");
        assert_eq!(task.total_step_duration_ms(), 0);
    }

    #[test]
    fn priority_serde_roundtrip() {
        for p in [
            TaskPriority::Low,
            TaskPriority::Normal,
            TaskPriority::High,
            TaskPriority::Critical,
        ] {
            let json = serde_json::to_string(&p).unwrap();
            let parsed: TaskPriority = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, p);
        }
    }
}
