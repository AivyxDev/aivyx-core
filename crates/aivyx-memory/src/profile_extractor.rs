//! Profile extraction from accumulated memory facts using the LLM.
//!
//! Reads all [`Fact`](crate::MemoryKind::Fact) and
//! [`Preference`](crate::MemoryKind::Preference) memories, sends them to the
//! LLM with a structured extraction prompt, and produces a merged
//! [`UserProfile`].

use aivyx_core::Result;
use aivyx_llm::{ChatMessage, ChatRequest};

use crate::profile::{ProjectEntry, RecurringTask, UserProfile};

/// System prompt for profile extraction.
const PROFILE_EXTRACTION_PROMPT: &str = r#"You are a profile extraction system. Given a list of facts and preferences learned about the user, produce a structured JSON profile.

Current profile (merge new information — do NOT discard existing data unless a fact explicitly contradicts it):
{current_profile}

Respond with ONLY a JSON object (no markdown, no explanation):
{
  "name": "string or null",
  "timezone": "IANA timezone string or null",
  "projects": [{"name": "...", "description": "...", "language": "...", "path": "..."}],
  "tech_stack": ["language1", "framework1"],
  "style_preferences": ["preference1"],
  "recurring_tasks": [{"description": "...", "frequency": "..."}],
  "schedule_hints": ["hint1"],
  "notes": ["anything that doesn't fit above"]
}

Rules:
- MERGE with existing profile data. Do not drop fields that already have values unless a fact explicitly contradicts them.
- If a fact corrects something in the current profile, update it.
- Only include information that was actually stated by the user. Do not infer or speculate.
- For tech_stack, include languages, frameworks, and tools.
- For style_preferences, include communication style, formatting preferences, etc.
- Use null for fields with no data.
- Omit empty arrays.
"#;

/// Build a [`ChatRequest`] for profile extraction.
pub fn build_profile_extraction_request(
    current: &UserProfile,
    facts_and_prefs: &[String],
) -> ChatRequest {
    let current_json = serde_json::to_string_pretty(current).unwrap_or_else(|_| "{}".to_string());

    let system_prompt = PROFILE_EXTRACTION_PROMPT.replace("{current_profile}", &current_json);

    let facts_text = facts_and_prefs
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{}. {f}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");

    ChatRequest {
        system_prompt: Some(system_prompt),
        messages: vec![ChatMessage::user(format!(
            "Extract a user profile from these facts and preferences:\n\n{facts_text}"
        ))],
        tools: vec![],
        model: None,
        max_tokens: 2048,
    }
}

/// Parse the LLM response into a [`UserProfile`], merging with the current
/// profile. Uses lenient parsing — unparseable fields are skipped, and the
/// existing profile is preserved for any missing fields.
pub fn parse_profile_extraction(response_text: &str, current: &UserProfile) -> Result<UserProfile> {
    let text = strip_markdown_fences(response_text);

    let extracted: serde_json::Value =
        serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({}));

    let mut profile = current.clone();

    // Scalar fields — LLM output takes precedence for non-null values
    if let Some(name) = extracted["name"].as_str() {
        profile.name = Some(name.to_string());
    }
    if let Some(tz) = extracted["timezone"].as_str() {
        profile.timezone = Some(tz.to_string());
    }

    // List fields — merge with dedup (case-insensitive)
    if let Some(arr) = extracted["tech_stack"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile
                .tech_stack
                .iter()
                .any(|s| s.eq_ignore_ascii_case(item))
            {
                profile.tech_stack.push(item.to_string());
            }
        }
    }

    if let Some(arr) = extracted["projects"].as_array() {
        for proj_val in arr {
            if let Some(name) = proj_val["name"].as_str() {
                let existing = profile
                    .projects
                    .iter_mut()
                    .find(|p| p.name.eq_ignore_ascii_case(name));
                match existing {
                    Some(p) => {
                        // Update existing project
                        if let Some(desc) = proj_val["description"].as_str() {
                            p.description = Some(desc.to_string());
                        }
                        if let Some(lang) = proj_val["language"].as_str() {
                            p.language = Some(lang.to_string());
                        }
                        if let Some(path) = proj_val["path"].as_str() {
                            p.path = Some(path.to_string());
                        }
                    }
                    None => {
                        profile.projects.push(ProjectEntry {
                            name: name.to_string(),
                            description: proj_val["description"].as_str().map(String::from),
                            language: proj_val["language"].as_str().map(String::from),
                            path: proj_val["path"].as_str().map(String::from),
                        });
                    }
                }
            }
        }
    }

    if let Some(arr) = extracted["style_preferences"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile
                .style_preferences
                .iter()
                .any(|s| s.eq_ignore_ascii_case(item))
            {
                profile.style_preferences.push(item.to_string());
            }
        }
    }

    if let Some(arr) = extracted["recurring_tasks"].as_array() {
        for task_val in arr {
            if let Some(desc) = task_val["description"].as_str()
                && !profile
                    .recurring_tasks
                    .iter()
                    .any(|t| t.description.eq_ignore_ascii_case(desc))
            {
                profile.recurring_tasks.push(RecurringTask {
                    description: desc.to_string(),
                    frequency: task_val["frequency"].as_str().map(String::from),
                });
            }
        }
    }

    if let Some(arr) = extracted["schedule_hints"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile
                .schedule_hints
                .iter()
                .any(|s| s.eq_ignore_ascii_case(item))
            {
                profile.schedule_hints.push(item.to_string());
            }
        }
    }

    if let Some(arr) = extracted["notes"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile.notes.contains(&item.to_string()) {
                profile.notes.push(item.to_string());
            }
        }
    }

    Ok(profile)
}

/// Strip markdown code fences (`` ```json ... ``` ``) if present.
fn strip_markdown_fences(text: &str) -> &str {
    let trimmed = text.trim();
    if trimmed.starts_with("```") {
        let after_fence = trimmed
            .find('\n')
            .map(|i| &trimmed[i + 1..])
            .unwrap_or(trimmed);
        after_fence
            .trim()
            .strip_suffix("```")
            .unwrap_or(after_fence)
            .trim()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_fields() {
        let json = r#"{
            "name": "Julian",
            "timezone": "America/New_York",
            "tech_stack": ["Rust", "TypeScript"],
            "projects": [{"name": "aivyx", "description": "AI framework", "language": "Rust"}],
            "style_preferences": ["concise"],
            "recurring_tasks": [{"description": "standup", "frequency": "daily"}],
            "schedule_hints": ["works evenings"],
            "notes": ["prefers dark mode"]
        }"#;

        let current = UserProfile::new();
        let result = parse_profile_extraction(json, &current).unwrap();

        assert_eq!(result.name.as_deref(), Some("Julian"));
        assert_eq!(result.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(result.tech_stack, vec!["Rust", "TypeScript"]);
        assert_eq!(result.projects.len(), 1);
        assert_eq!(result.projects[0].name, "aivyx");
        assert_eq!(result.style_preferences, vec!["concise"]);
        assert_eq!(result.recurring_tasks.len(), 1);
        assert_eq!(result.schedule_hints, vec!["works evenings"]);
        assert_eq!(result.notes, vec!["prefers dark mode"]);
    }

    #[test]
    fn parse_merges_with_existing() {
        let mut current = UserProfile::new();
        current.name = Some("Julian".into());
        current.tech_stack = vec!["Rust".into()];
        current.projects.push(ProjectEntry {
            name: "aivyx".into(),
            description: None,
            language: Some("Rust".into()),
            path: None,
        });

        let json = r#"{
            "tech_stack": ["Rust", "TypeScript"],
            "projects": [{"name": "aivyx", "description": "AI framework"}]
        }"#;

        let result = parse_profile_extraction(json, &current).unwrap();

        // Name preserved from current
        assert_eq!(result.name.as_deref(), Some("Julian"));
        // Rust not duplicated, TypeScript added
        assert_eq!(result.tech_stack, vec!["Rust", "TypeScript"]);
        // aivyx project updated with description, language preserved
        assert_eq!(result.projects.len(), 1);
        assert_eq!(
            result.projects[0].description.as_deref(),
            Some("AI framework")
        );
        assert_eq!(result.projects[0].language.as_deref(), Some("Rust"));
    }

    #[test]
    fn parse_handles_markdown_fences() {
        let text = "```json\n{\"name\": \"Julian\"}\n```";
        let current = UserProfile::new();
        let result = parse_profile_extraction(text, &current).unwrap();
        assert_eq!(result.name.as_deref(), Some("Julian"));
    }

    #[test]
    fn parse_handles_partial_response() {
        let json = r#"{"tech_stack": ["Go"]}"#;
        let mut current = UserProfile::new();
        current.name = Some("Julian".into());

        let result = parse_profile_extraction(json, &current).unwrap();
        assert_eq!(result.name.as_deref(), Some("Julian"));
        assert_eq!(result.tech_stack, vec!["Go"]);
    }

    #[test]
    fn parse_handles_invalid_json() {
        let text = "This is not JSON at all!";
        let current = UserProfile::new();
        let result = parse_profile_extraction(text, &current).unwrap();
        // Should return current profile unchanged
        assert!(result.is_empty());
    }

    #[test]
    fn build_request_includes_current_profile() {
        let mut current = UserProfile::new();
        current.name = Some("Julian".into());

        let facts = vec!["User prefers Rust".to_string()];
        let request = build_profile_extraction_request(&current, &facts);

        let prompt = request.system_prompt.unwrap();
        assert!(prompt.contains("Julian"));
        assert_eq!(request.messages.len(), 1);
        assert!(request.messages[0].content.contains("User prefers Rust"));
    }
}
