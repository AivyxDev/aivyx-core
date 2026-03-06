//! Structured user profile that accumulates over time from conversations.
//!
//! The `UserProfile` is a singleton per aivyx installation (single-user
//! assumption). It lives in the same encrypted store as memories but is stored
//! as a separate document, not in the vector index.
//!
//! Profile data is built from accumulated [`Fact`](crate::MemoryKind::Fact) and
//! [`Preference`](crate::MemoryKind::Preference) memories via LLM extraction,
//! and injected into agent system prompts as a `[USER PROFILE]` block.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current schema version. Bump when fields are added or removed, and add a
/// migration path when needed.
pub const PROFILE_VERSION: u32 = 1;

/// A structured user profile that accumulates over time.
///
/// Stored encrypted in `MemoryStore` under key `"profile:current"`, with
/// versioned snapshots under `"profile:v{revision}"` before each update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// Schema version for forward-compatible migrations.
    pub version: u32,

    /// User's preferred name.
    #[serde(default)]
    pub name: Option<String>,

    /// IANA timezone (e.g., `"America/New_York"`).
    #[serde(default)]
    pub timezone: Option<String>,

    /// Active projects the user works on.
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,

    /// Programming languages, frameworks, and tools the user uses.
    #[serde(default)]
    pub tech_stack: Vec<String>,

    /// Communication style preferences (e.g., "concise", "prefer code
    /// examples", "no emojis").
    #[serde(default)]
    pub style_preferences: Vec<String>,

    /// Recurring tasks or routines (e.g., "weekly standup prep").
    #[serde(default)]
    pub recurring_tasks: Vec<RecurringTask>,

    /// Work schedule hints (e.g., "works late evenings").
    #[serde(default)]
    pub schedule_hints: Vec<String>,

    /// Freeform notes that don't fit the structured fields.
    #[serde(default)]
    pub notes: Vec<String>,

    /// When this profile was first created.
    pub created_at: DateTime<Utc>,

    /// When this profile was last updated.
    pub updated_at: DateTime<Utc>,

    /// How many times the profile has been updated (monotonically increasing).
    pub revision: u64,
}

/// A project the user is actively working on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntry {
    /// Project name (e.g., "aivyx").
    pub name: String,
    /// Short description.
    #[serde(default)]
    pub description: Option<String>,
    /// Primary language or framework.
    #[serde(default)]
    pub language: Option<String>,
    /// Filesystem path, if known.
    #[serde(default)]
    pub path: Option<String>,
}

/// A recurring task or routine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecurringTask {
    /// What the task is.
    pub description: String,
    /// How often (e.g., "daily", "weekly", "every Monday").
    #[serde(default)]
    pub frequency: Option<String>,
}

impl UserProfile {
    /// Create an empty profile with the current timestamp.
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            version: PROFILE_VERSION,
            name: None,
            timezone: None,
            projects: Vec::new(),
            tech_stack: Vec::new(),
            style_preferences: Vec::new(),
            recurring_tasks: Vec::new(),
            schedule_hints: Vec::new(),
            notes: Vec::new(),
            created_at: now,
            updated_at: now,
            revision: 0,
        }
    }

    /// Whether the profile has any meaningful content.
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.timezone.is_none()
            && self.projects.is_empty()
            && self.tech_stack.is_empty()
            && self.style_preferences.is_empty()
            && self.recurring_tasks.is_empty()
            && self.schedule_hints.is_empty()
            && self.notes.is_empty()
    }

    /// Format the profile as a `[USER PROFILE]...[END USER PROFILE]` block
    /// suitable for system prompt injection.
    ///
    /// Returns `None` if the profile is empty.
    pub fn format_for_prompt(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }

        let mut sections = Vec::new();

        if let Some(ref name) = self.name {
            sections.push(format!("User's name: {name}"));
        }
        if let Some(ref tz) = self.timezone {
            sections.push(format!("Timezone: {tz}"));
        }
        if !self.tech_stack.is_empty() {
            sections.push(format!("Tech stack: {}", self.tech_stack.join(", ")));
        }
        if !self.projects.is_empty() {
            let project_list: Vec<String> = self
                .projects
                .iter()
                .map(|p| {
                    let mut s = p.name.clone();
                    if let Some(ref desc) = p.description {
                        s.push_str(&format!(" ({desc})"));
                    }
                    if let Some(ref lang) = p.language {
                        s.push_str(&format!(" [{lang}]"));
                    }
                    s
                })
                .collect();
            sections.push(format!("Active projects: {}", project_list.join("; ")));
        }
        if !self.style_preferences.is_empty() {
            sections.push(format!(
                "Communication preferences: {}",
                self.style_preferences.join(", ")
            ));
        }
        if !self.recurring_tasks.is_empty() {
            let task_list: Vec<String> = self
                .recurring_tasks
                .iter()
                .map(|t| {
                    if let Some(ref freq) = t.frequency {
                        format!("{} ({})", t.description, freq)
                    } else {
                        t.description.clone()
                    }
                })
                .collect();
            sections.push(format!("Recurring tasks: {}", task_list.join("; ")));
        }
        if !self.schedule_hints.is_empty() {
            sections.push(format!("Schedule: {}", self.schedule_hints.join(", ")));
        }

        if sections.is_empty() {
            return None;
        }

        Some(format!(
            "[USER PROFILE]\n{}\n[END USER PROFILE]",
            sections.join("\n")
        ))
    }
}

impl Default for UserProfile {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_profile() {
        let profile = UserProfile::new();
        assert_eq!(profile.version, PROFILE_VERSION);
        assert!(profile.name.is_none());
        assert!(profile.timezone.is_none());
        assert!(profile.projects.is_empty());
        assert!(profile.tech_stack.is_empty());
        assert!(profile.style_preferences.is_empty());
        assert!(profile.recurring_tasks.is_empty());
        assert!(profile.schedule_hints.is_empty());
        assert!(profile.notes.is_empty());
        assert_eq!(profile.revision, 0);
    }

    #[test]
    fn default_matches_new() {
        let default = UserProfile::default();
        assert_eq!(default.version, PROFILE_VERSION);
        assert!(default.is_empty());
        assert_eq!(default.revision, 0);
    }

    #[test]
    fn serde_roundtrip() {
        let mut profile = UserProfile::new();
        profile.name = Some("Julian".into());
        profile.timezone = Some("America/New_York".into());
        profile.tech_stack = vec!["Rust".into(), "TypeScript".into()];
        profile.projects.push(ProjectEntry {
            name: "aivyx".into(),
            description: Some("AI agent framework".into()),
            language: Some("Rust".into()),
            path: Some("/home/julian/Projects/Personal-AI".into()),
        });
        profile.style_preferences = vec!["concise".into()];
        profile.recurring_tasks.push(RecurringTask {
            description: "morning review".into(),
            frequency: Some("daily".into()),
        });
        profile.schedule_hints = vec!["works evenings".into()];
        profile.notes = vec!["prefers dark mode".into()];
        profile.revision = 5;

        let json = serde_json::to_string(&profile).unwrap();
        let parsed: UserProfile = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name.as_deref(), Some("Julian"));
        assert_eq!(parsed.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(parsed.tech_stack, vec!["Rust", "TypeScript"]);
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].name, "aivyx");
        assert_eq!(parsed.projects[0].language.as_deref(), Some("Rust"));
        assert_eq!(parsed.style_preferences, vec!["concise"]);
        assert_eq!(parsed.recurring_tasks.len(), 1);
        assert_eq!(
            parsed.recurring_tasks[0].frequency.as_deref(),
            Some("daily")
        );
        assert_eq!(parsed.schedule_hints, vec!["works evenings"]);
        assert_eq!(parsed.notes, vec!["prefers dark mode"]);
        assert_eq!(parsed.revision, 5);
    }

    #[test]
    fn is_empty_on_new() {
        assert!(UserProfile::new().is_empty());
    }

    #[test]
    fn is_empty_false_with_name() {
        let mut profile = UserProfile::new();
        profile.name = Some("Julian".into());
        assert!(!profile.is_empty());
    }

    #[test]
    fn is_empty_false_with_tech_stack() {
        let mut profile = UserProfile::new();
        profile.tech_stack = vec!["Rust".into()];
        assert!(!profile.is_empty());
    }

    #[test]
    fn format_for_prompt_empty_returns_none() {
        let profile = UserProfile::new();
        assert!(profile.format_for_prompt().is_none());
    }

    #[test]
    fn format_for_prompt_with_data() {
        let mut profile = UserProfile::new();
        profile.name = Some("Julian".into());
        profile.timezone = Some("America/New_York".into());

        let output = profile.format_for_prompt().unwrap();
        assert!(output.starts_with("[USER PROFILE]"));
        assert!(output.ends_with("[END USER PROFILE]"));
        assert!(output.contains("User's name: Julian"));
        assert!(output.contains("Timezone: America/New_York"));
    }

    #[test]
    fn format_for_prompt_includes_all_sections() {
        let mut profile = UserProfile::new();
        profile.name = Some("Julian".into());
        profile.timezone = Some("UTC".into());
        profile.tech_stack = vec!["Rust".into(), "TypeScript".into()];
        profile.projects.push(ProjectEntry {
            name: "aivyx".into(),
            description: Some("AI framework".into()),
            language: Some("Rust".into()),
            path: None,
        });
        profile.style_preferences = vec!["concise".into()];
        profile.recurring_tasks.push(RecurringTask {
            description: "standup".into(),
            frequency: Some("daily".into()),
        });
        profile.schedule_hints = vec!["works evenings".into()];

        let output = profile.format_for_prompt().unwrap();
        assert!(output.contains("User's name: Julian"));
        assert!(output.contains("Timezone: UTC"));
        assert!(output.contains("Tech stack: Rust, TypeScript"));
        assert!(output.contains("Active projects: aivyx (AI framework) [Rust]"));
        assert!(output.contains("Communication preferences: concise"));
        assert!(output.contains("Recurring tasks: standup (daily)"));
        assert!(output.contains("Schedule: works evenings"));
    }

    #[test]
    fn serde_partial_fields_ok() {
        let json = r#"{"version":1,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","revision":0}"#;
        let profile: UserProfile = serde_json::from_str(json).unwrap();
        assert!(profile.name.is_none());
        assert!(profile.tech_stack.is_empty());
        assert!(profile.is_empty());
    }

    #[test]
    fn project_entry_serde() {
        let entry = ProjectEntry {
            name: "test".into(),
            description: Some("a test project".into()),
            language: None,
            path: Some("/tmp/test".into()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ProjectEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.description.as_deref(), Some("a test project"));
        assert!(parsed.language.is_none());
        assert_eq!(parsed.path.as_deref(), Some("/tmp/test"));
    }
}
