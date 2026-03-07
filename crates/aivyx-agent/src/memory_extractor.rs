//! Post-turn memory extraction using the LLM.
//!
//! After each conversation turn, this module asks the LLM to identify:
//! - Facts about the user or their world
//! - Preferences the user expressed
//! - Knowledge triples (subject–predicate–object)
//!
//! Extracted items are stored in the memory manager asynchronously.

use serde::Deserialize;
use tracing::{debug, info, warn};

use aivyx_core::{AgentId, Result};
use aivyx_llm::{ChatMessage, ChatRequest, LlmProvider, Role};

/// Extraction results parsed from the LLM response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExtractionResult {
    /// Facts about the user or their context.
    #[serde(default)]
    pub facts: Vec<String>,
    /// User preferences and opinions.
    #[serde(default)]
    pub preferences: Vec<String>,
    /// Knowledge triples: (subject, predicate, object).
    #[serde(default)]
    pub triples: Vec<TripleExtraction>,
    /// Corrections to previously stored information.
    #[serde(default)]
    pub corrections: Vec<CorrectionExtraction>,
}

/// A single knowledge triple extracted from conversation.
#[derive(Debug, Clone, Deserialize)]
pub struct TripleExtraction {
    pub subject: String,
    pub predicate: String,
    pub object: String,
}

/// A correction detected when the user contradicts previously stored data.
///
/// For example: "No, I use Python not Go" → `{ incorrect: "Go", correct: "Python", field_hint: Some("tech_stack") }`.
#[derive(Debug, Clone, Deserialize)]
pub struct CorrectionExtraction {
    /// The incorrect information that should be removed or updated.
    pub incorrect: String,
    /// The correct information that should replace it.
    pub correct: String,
    /// Optional hint about which profile field this correction targets
    /// (e.g., `"tech_stack"`, `"name"`, `"timezone"`).
    #[serde(default)]
    pub field_hint: Option<String>,
}

/// The extraction prompt that instructs the LLM on what to extract.
const EXTRACTION_PROMPT: &str = r#"You are a memory extraction system. Your job is to identify important facts, preferences, knowledge, and corrections from the conversation below.

Extract ONLY information that would be useful to remember for future conversations. Focus on:
- Facts about the user (name, location, projects, skills, habits)
- Preferences (likes, dislikes, preferred tools, communication style)
- Knowledge triples (relationships between concepts the user cares about)
- Corrections — when the user says "no, I meant X", "actually it's Y", or "I don't use Z anymore"

Do NOT extract:
- Transient task details (specific file paths being edited, current debugging state)
- Things the AI said (only extract what the USER revealed)
- Generic facts unrelated to the user's personal context

Respond with ONLY a JSON object (no markdown, no explanation):
{
  "facts": ["fact1", "fact2"],
  "preferences": ["preference1"],
  "triples": [{"subject": "X", "predicate": "uses", "object": "Y"}],
  "corrections": [{"incorrect": "what was wrong", "correct": "what is true", "field_hint": "tech_stack or null"}]
}

If there's nothing worth remembering, respond with:
{"facts": [], "preferences": [], "triples": [], "corrections": []}
"#;

/// Extract memories from the most recent conversation exchange.
///
/// Takes the last N messages from the conversation (user + assistant) and asks
/// the LLM to identify memorable facts, preferences, and triples.
pub async fn extract_from_turn(
    provider: &dyn LlmProvider,
    conversation: &[ChatMessage],
    max_messages: usize,
) -> Result<ExtractionResult> {
    // Take the last N messages (typically 2–4: user + assistant exchanges)
    let recent: Vec<&ChatMessage> = conversation.iter().rev().take(max_messages).collect();
    if recent.is_empty() {
        return Ok(ExtractionResult::default());
    }

    // Format the conversation excerpt
    let mut excerpt = String::new();
    for msg in recent.iter().rev() {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System | Role::Tool => continue,
        };
        excerpt.push_str(&format!("{role_label}: {}\n\n", msg.content));
    }

    if excerpt.trim().is_empty() {
        return Ok(ExtractionResult::default());
    }

    // Ask the LLM to extract
    let request = ChatRequest {
        system_prompt: Some(EXTRACTION_PROMPT.to_string()),
        messages: vec![ChatMessage::user(format!(
            "Extract memories from this conversation:\n\n{excerpt}"
        ))],
        tools: vec![],
        model: None,
        max_tokens: 1024,
    };

    let response = provider.chat(&request).await?;
    let text = response.message.content.text().trim();

    // Parse JSON — be tolerant of markdown fences
    let json_text = strip_markdown_fences(text);

    match serde_json::from_str::<ExtractionResult>(json_text) {
        Ok(result) => {
            let total = result.facts.len() + result.preferences.len() + result.triples.len();
            if total > 0 {
                info!(
                    "Extracted {} facts, {} preferences, {} triples",
                    result.facts.len(),
                    result.preferences.len(),
                    result.triples.len()
                );
            }
            Ok(result)
        }
        Err(e) => {
            debug!("Failed to parse extraction result: {e} — text: {json_text}");
            Ok(ExtractionResult::default())
        }
    }
}

/// Store extraction results in the memory manager.
///
/// Deduplication is handled automatically by `MemoryManager::remember()` —
/// near-duplicate facts and preferences (cosine similarity ≥ 0.95) are
/// silently skipped.
#[cfg(feature = "memory")]
pub async fn store_extractions(
    memory_manager: &std::sync::Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>,
    result: &ExtractionResult,
    agent_id: Option<AgentId>,
) {
    use aivyx_memory::MemoryKind;

    let mut mgr = memory_manager.lock().await;
    let mut stored = 0usize;
    let mut failed = 0usize;

    for fact in &result.facts {
        match mgr
            .remember(
                fact.clone(),
                MemoryKind::Fact,
                agent_id,
                vec!["extracted".into()],
            )
            .await
        {
            Ok(_) => stored += 1,
            Err(e) => {
                warn!("Failed to store extracted fact: {e}");
                failed += 1;
            }
        }
    }

    for pref in &result.preferences {
        match mgr
            .remember(
                pref.clone(),
                MemoryKind::Preference,
                agent_id,
                vec!["extracted".into()],
            )
            .await
        {
            Ok(_) => stored += 1,
            Err(e) => {
                warn!("Failed to store extracted preference: {e}");
                failed += 1;
            }
        }
    }

    for triple in &result.triples {
        match mgr.add_triple(
            triple.subject.clone(),
            triple.predicate.clone(),
            triple.object.clone(),
            agent_id,
            0.8, // default confidence for extraction
            "auto-extract".into(),
        ) {
            Ok(_) => stored += 1,
            Err(e) => {
                warn!("Failed to store extracted triple: {e}");
                failed += 1;
            }
        }
    }

    // Process corrections: store as facts and apply to profile
    for correction in &result.corrections {
        // Store the correction as a fact tagged "correction" for future extraction
        let fact_text = format!(
            "Correction: not \"{}\", actually \"{}\"",
            correction.incorrect, correction.correct
        );
        match mgr
            .remember(
                fact_text,
                MemoryKind::Fact,
                agent_id,
                vec!["extracted".into(), "correction".into()],
            )
            .await
        {
            Ok(_) => stored += 1,
            Err(e) => {
                warn!("Failed to store correction fact: {e}");
                failed += 1;
            }
        }

        // Immediately apply correction to profile
        if let Err(e) = apply_correction_to_profile(&mgr, correction) {
            warn!("Failed to apply correction to profile: {e}");
        }
    }

    // Increment extraction counter for profile extraction threshold
    let fact_count = result.facts.len() + result.corrections.len();
    if fact_count > 0 {
        match mgr.increment_extraction_counter() {
            Ok(counter) => {
                debug!("Extraction counter: {counter}");
            }
            Err(e) => {
                warn!("Failed to increment extraction counter: {e}");
            }
        }
    }

    let total = result.facts.len()
        + result.preferences.len()
        + result.triples.len()
        + result.corrections.len();
    if total > 0 {
        info!("Extraction: {stored} stored/deduped, {failed} failed (of {total} items)");
    }
}

/// Apply a correction to the user profile by heuristically matching the
/// incorrect value against profile fields and replacing it with the correct
/// value.
///
/// If a `field_hint` is provided, only that field is checked. Otherwise,
/// all list fields are scanned with a case-insensitive match.
#[cfg(feature = "memory")]
fn apply_correction_to_profile(
    mgr: &aivyx_memory::MemoryManager,
    correction: &CorrectionExtraction,
) -> aivyx_core::Result<()> {
    let mut profile = mgr.get_profile()?;
    let mut changed = false;

    let fields_to_check: Vec<&str> = match correction.field_hint.as_deref() {
        Some(hint) => vec![hint],
        None => vec![
            "name",
            "timezone",
            "tech_stack",
            "style_preferences",
            "schedule_hints",
            "notes",
        ],
    };

    for field in &fields_to_check {
        match *field {
            "name" => {
                if let Some(ref name) = profile.name
                    && name.eq_ignore_ascii_case(&correction.incorrect)
                {
                    profile.name = Some(correction.correct.clone());
                    changed = true;
                }
            }
            "timezone" => {
                if let Some(ref tz) = profile.timezone
                    && tz.eq_ignore_ascii_case(&correction.incorrect)
                {
                    profile.timezone = Some(correction.correct.clone());
                    changed = true;
                }
            }
            "tech_stack" => {
                if let Some(pos) = profile
                    .tech_stack
                    .iter()
                    .position(|s| s.eq_ignore_ascii_case(&correction.incorrect))
                {
                    profile.tech_stack[pos] = correction.correct.clone();
                    changed = true;
                }
            }
            "style_preferences" => {
                if let Some(pos) = profile
                    .style_preferences
                    .iter()
                    .position(|s| s.eq_ignore_ascii_case(&correction.incorrect))
                {
                    profile.style_preferences[pos] = correction.correct.clone();
                    changed = true;
                }
            }
            "schedule_hints" => {
                if let Some(pos) = profile
                    .schedule_hints
                    .iter()
                    .position(|s| s.eq_ignore_ascii_case(&correction.incorrect))
                {
                    profile.schedule_hints[pos] = correction.correct.clone();
                    changed = true;
                }
            }
            "notes" => {
                if let Some(pos) = profile
                    .notes
                    .iter()
                    .position(|s| s.eq_ignore_ascii_case(&correction.incorrect))
                {
                    profile.notes[pos] = correction.correct.clone();
                    changed = true;
                }
            }
            _ => {}
        }
    }

    if changed {
        info!(
            "Applied correction: \"{}\" → \"{}\"",
            correction.incorrect, correction.correct
        );
        mgr.update_profile(profile)?;
    }

    Ok(())
}

/// Prompt for extracting knowledge triples from session summaries.
///
/// Unlike `EXTRACTION_PROMPT`, this focuses exclusively on entity relationships
/// (triples) rather than facts/preferences — those are already captured per-turn.
const SUMMARY_EXTRACTION_PROMPT: &str = r#"You are a knowledge graph extraction system. Extract entity relationships (subject-predicate-object triples) from the session summary below.

Focus on:
- Who/what is related to what (e.g., "Julian" - "works_on" - "Aivyx")
- Tools/technologies and their relationships (e.g., "Aivyx" - "uses" - "Rust")
- Decisions and their subjects (e.g., "team" - "chose" - "PostgreSQL")
- Project relationships (e.g., "aivyx-core" - "depends_on" - "aivyx-crypto")

Do NOT extract:
- Transient details (specific commands run, temporary file paths)
- Subjective opinions or preferences (already captured elsewhere)

Respond with ONLY a JSON object (no markdown, no explanation):
{"triples": [{"subject": "X", "predicate": "relation", "object": "Y"}]}

If there are no meaningful relationships, respond with:
{"triples": []}
"#;

/// Response shape for summary extraction — triples only.
#[derive(Debug, Deserialize)]
struct SummaryExtractionResult {
    #[serde(default)]
    triples: Vec<TripleExtraction>,
}

/// Extract knowledge triples from a session summary.
///
/// Unlike [`extract_from_turn()`] which extracts from raw conversation, this
/// operates on the condensed session summary and focuses exclusively on triples
/// (entity relationships) rather than facts/preferences (which were already
/// extracted per-turn during the session).
pub async fn extract_from_summary(
    provider: &dyn LlmProvider,
    summary: &str,
) -> Result<Vec<TripleExtraction>> {
    if summary.trim().is_empty() {
        return Ok(Vec::new());
    }

    let request = ChatRequest {
        system_prompt: Some(SUMMARY_EXTRACTION_PROMPT.to_string()),
        messages: vec![ChatMessage::user(format!(
            "Extract entity relationships from this session summary:\n\n{summary}"
        ))],
        tools: vec![],
        model: None,
        max_tokens: 512,
    };

    let response = provider.chat(&request).await?;
    let text = response.message.content.text().trim();
    let json_text = strip_markdown_fences(text);

    match serde_json::from_str::<SummaryExtractionResult>(json_text) {
        Ok(result) => {
            if !result.triples.is_empty() {
                info!(
                    "Extracted {} triples from session summary",
                    result.triples.len()
                );
            }
            Ok(result.triples)
        }
        Err(e) => {
            debug!("Failed to parse summary extraction result: {e} — text: {json_text}");
            Ok(Vec::new())
        }
    }
}

/// Strip markdown code fences (```json ... ```) if present.
fn strip_markdown_fences(text: &str) -> &str {
    let trimmed = text.trim();
    if trimmed.starts_with("```") {
        // Find end of first line (the opening fence)
        let after_fence = trimmed
            .find('\n')
            .map(|i| &trimmed[i + 1..])
            .unwrap_or(trimmed);
        // Strip closing fence
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
    fn parse_extraction_result() {
        let json = r#"{
            "facts": ["User's name is Julian", "User works on the Aivyx project"],
            "preferences": ["User prefers Rust over Python"],
            "triples": [
                {"subject": "Julian", "predicate": "works_on", "object": "Aivyx"},
                {"subject": "Julian", "predicate": "prefers", "object": "Rust"}
            ]
        }"#;

        let result: ExtractionResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.facts.len(), 2);
        assert_eq!(result.preferences.len(), 1);
        assert_eq!(result.triples.len(), 2);
        assert_eq!(result.triples[0].subject, "Julian");
    }

    #[test]
    fn parse_empty_result() {
        let json = r#"{"facts": [], "preferences": [], "triples": []}"#;
        let result: ExtractionResult = serde_json::from_str(json).unwrap();
        assert!(result.facts.is_empty());
        assert!(result.preferences.is_empty());
        assert!(result.triples.is_empty());
    }

    #[test]
    fn strip_fences_json() {
        let text = "```json\n{\"facts\": []}\n```";
        assert_eq!(strip_markdown_fences(text), "{\"facts\": []}");
    }

    #[test]
    fn strip_fences_plain() {
        let text = "{\"facts\": []}";
        assert_eq!(strip_markdown_fences(text), "{\"facts\": []}");
    }

    #[test]
    fn partial_fields_ok() {
        let json = r#"{"facts": ["something"]}"#;
        let result: ExtractionResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.facts.len(), 1);
        assert!(result.preferences.is_empty());
        assert!(result.triples.is_empty());
        assert!(result.corrections.is_empty());
    }

    #[test]
    fn parse_with_corrections() {
        let json = r#"{
            "facts": ["User likes Rust"],
            "preferences": [],
            "triples": [],
            "corrections": [
                {"incorrect": "Go", "correct": "Python", "field_hint": "tech_stack"},
                {"incorrect": "UTC", "correct": "America/New_York"}
            ]
        }"#;

        let result: ExtractionResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.corrections.len(), 2);
        assert_eq!(result.corrections[0].incorrect, "Go");
        assert_eq!(result.corrections[0].correct, "Python");
        assert_eq!(
            result.corrections[0].field_hint.as_deref(),
            Some("tech_stack")
        );
        assert!(result.corrections[1].field_hint.is_none());
    }

    #[test]
    fn parse_without_corrections_backward_compat() {
        // Ensure old-format JSON (without corrections field) still parses
        let json = r#"{
            "facts": ["User likes Rust"],
            "preferences": ["prefers dark mode"],
            "triples": [{"subject": "User", "predicate": "likes", "object": "Rust"}]
        }"#;

        let result: ExtractionResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.facts.len(), 1);
        assert_eq!(result.preferences.len(), 1);
        assert_eq!(result.triples.len(), 1);
        assert!(result.corrections.is_empty());
    }

    #[test]
    fn parse_summary_extraction_result() {
        let json = r#"{
            "triples": [
                {"subject": "Julian", "predicate": "works_on", "object": "Aivyx"},
                {"subject": "Aivyx", "predicate": "uses", "object": "Rust"},
                {"subject": "aivyx-core", "predicate": "depends_on", "object": "aivyx-crypto"}
            ]
        }"#;

        let result: SummaryExtractionResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.triples.len(), 3);
        assert_eq!(result.triples[0].subject, "Julian");
        assert_eq!(result.triples[0].predicate, "works_on");
        assert_eq!(result.triples[0].object, "Aivyx");
        assert_eq!(result.triples[2].predicate, "depends_on");
    }

    #[test]
    fn parse_summary_extraction_empty() {
        let json = r#"{"triples": []}"#;
        let result: SummaryExtractionResult = serde_json::from_str(json).unwrap();
        assert!(result.triples.is_empty());
    }

    #[test]
    fn summary_extraction_prompt_contains_key_instructions() {
        assert!(SUMMARY_EXTRACTION_PROMPT.contains("knowledge graph"));
        assert!(SUMMARY_EXTRACTION_PROMPT.contains("subject"));
        assert!(SUMMARY_EXTRACTION_PROMPT.contains("predicate"));
        assert!(SUMMARY_EXTRACTION_PROMPT.contains("object"));
        assert!(SUMMARY_EXTRACTION_PROMPT.contains("triples"));
        // Should NOT ask for facts/preferences (already captured per-turn)
        assert!(!SUMMARY_EXTRACTION_PROMPT.contains("\"facts\""));
        assert!(!SUMMARY_EXTRACTION_PROMPT.contains("\"preferences\""));
    }
}
