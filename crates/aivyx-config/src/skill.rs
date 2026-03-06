//! SKILL.md parser and skill discovery.
//!
//! Implements the [Agent Skills specification](https://agentskills.io/specification)
//! for consuming SKILL.md files. Skills are structured markdown files with YAML
//! frontmatter that encode procedural knowledge for agents.
//!
//! The loading follows a three-tier progressive disclosure model:
//! - **Tier 1 (Discovery)**: Only `name` + `description` from frontmatter (~50 tokens/skill)
//! - **Tier 2 (Activation)**: Full markdown body loaded on demand (500-5000 tokens/skill)
//! - **Tier 3 (Execution)**: Referenced files from `scripts/`, `references/` loaded as needed

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aivyx_core::{AivyxError, Result};
use serde::{Deserialize, Serialize};

/// Maximum allowed length for a skill name.
const MAX_NAME_LEN: usize = 64;

/// Maximum allowed length for a skill description.
const MAX_DESCRIPTION_LEN: usize = 1024;

/// Parsed YAML frontmatter from a SKILL.md file.
///
/// Follows the [Agent Skills specification](https://agentskills.io/specification).
/// Required fields are `name` and `description`; all others are optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Unique name (max 64 chars, lowercase letters, numbers, hyphens).
    pub name: String,
    /// Human-readable description loaded at Tier 1 (max 1024 chars).
    pub description: String,
    /// SPDX license identifier (e.g., `"MIT"`, `"Apache-2.0"`).
    #[serde(default)]
    pub license: Option<String>,
    /// Environment requirements (e.g., `"Requires Node.js >=18"`).
    #[serde(default)]
    pub compatibility: Option<String>,
    /// Space-delimited tool allowlist (e.g., `"Bash(git:*) Read Write"`).
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<String>,
    /// Extension metadata (author, version, tags, etc.).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Lightweight Tier-1 summary for system prompt injection at startup.
///
/// Contains only the information needed to present available skills to the
/// agent without loading full skill bodies.
#[derive(Debug, Clone)]
pub struct SkillSummary {
    /// The skill's unique name.
    pub name: String,
    /// Brief description for the agent to decide relevance.
    pub description: String,
    /// Path to the SKILL.md file (for Tier-2 loading).
    pub path: PathBuf,
}

/// A fully loaded skill (Tier 2).
///
/// Contains the parsed frontmatter plus the full markdown body with
/// instructions the agent should follow when the skill is activated.
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    /// Parsed YAML frontmatter.
    pub manifest: SkillManifest,
    /// Markdown body (everything after the closing `---` delimiter).
    pub body: String,
    /// Directory containing the SKILL.md file (for resolving relative paths
    /// to `scripts/`, `references/`, `assets/`).
    pub base_dir: PathBuf,
}

impl SkillManifest {
    /// Parse a SKILL.md file's content into a manifest and body.
    ///
    /// The file must start with `---`, contain YAML frontmatter, then a
    /// closing `---`, followed by the markdown body.
    ///
    /// # Errors
    ///
    /// Returns an error if the frontmatter delimiters are missing, the YAML
    /// is invalid, or required fields (`name`, `description`) are absent.
    pub fn parse(content: &str) -> Result<(Self, String)> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return Err(AivyxError::Config(
                "SKILL.md must start with '---' frontmatter delimiter".into(),
            ));
        }

        // Find the closing delimiter
        let after_first = &trimmed[3..];
        let after_first = after_first.trim_start_matches(['\r', '\n']);
        let closing_pos = after_first.find("\n---").ok_or_else(|| {
            AivyxError::Config("SKILL.md missing closing '---' frontmatter delimiter".into())
        })?;

        let yaml_str = &after_first[..closing_pos];
        let body_start = closing_pos + 4; // skip "\n---"
        let body = if body_start < after_first.len() {
            after_first[body_start..].trim().to_string()
        } else {
            String::new()
        };

        let manifest: Self = serde_yaml::from_str(yaml_str)
            .map_err(|e| AivyxError::Config(format!("SKILL.md frontmatter YAML error: {e}")))?;

        manifest.validate()?;
        Ok((manifest, body))
    }

    /// Validate the manifest fields.
    ///
    /// Checks:
    /// - `name` is non-empty, max 64 chars, lowercase + hyphens + digits only
    /// - `description` is non-empty, max 1024 chars
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(AivyxError::Config("SKILL.md: name is required".into()));
        }
        if self.name.len() > MAX_NAME_LEN {
            return Err(AivyxError::Config(format!(
                "SKILL.md: name exceeds {} chars: '{}'",
                MAX_NAME_LEN, self.name
            )));
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(AivyxError::Config(format!(
                "SKILL.md: name must be lowercase letters, digits, and hyphens: '{}'",
                self.name
            )));
        }
        if self.description.is_empty() {
            return Err(AivyxError::Config(
                "SKILL.md: description is required".into(),
            ));
        }
        if self.description.len() > MAX_DESCRIPTION_LEN {
            return Err(AivyxError::Config(format!(
                "SKILL.md: description exceeds {} chars",
                MAX_DESCRIPTION_LEN
            )));
        }
        Ok(())
    }
}

/// Discover all SKILL.md files in a directory (one level deep).
///
/// Scans `dir/<name>/SKILL.md` entries, parsing only the YAML frontmatter
/// (Tier 1). Hidden directories (starting with `.`) are skipped.
///
/// Returns an empty vec (not an error) if the directory doesn't exist.
pub fn discover_skills(dir: &Path) -> Result<Vec<SkillSummary>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| {
        AivyxError::Config(format!(
            "cannot read skills directory '{}': {e}",
            dir.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AivyxError::Config(format!("error reading skills directory entry: {e}"))
        })?;
        let path = entry.path();

        // Skip non-directories and hidden directories
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        match parse_frontmatter_only(&skill_md) {
            Ok(summary) => summaries.push(summary),
            Err(e) => {
                tracing::warn!("Skipping invalid skill at '{}': {e}", skill_md.display());
            }
        }
    }

    // Sort by name for deterministic ordering
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summaries)
}

/// Load a skill fully (Tier 2): parse frontmatter + body.
///
/// Reads the SKILL.md file at `path`, parses the YAML frontmatter and
/// markdown body, validates, and returns a [`LoadedSkill`].
pub fn load_skill(path: &Path) -> Result<LoadedSkill> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        AivyxError::Config(format!("cannot read SKILL.md at '{}': {e}", path.display()))
    })?;

    let (manifest, body) = SkillManifest::parse(&content)?;

    let base_dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    Ok(LoadedSkill {
        manifest,
        body,
        base_dir,
    })
}

/// Parse only the frontmatter from a SKILL.md file (Tier 1).
fn parse_frontmatter_only(path: &Path) -> Result<SkillSummary> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        AivyxError::Config(format!("cannot read SKILL.md at '{}': {e}", path.display()))
    })?;

    let (manifest, _body) = SkillManifest::parse(&content)?;

    // Validate that the directory name matches the skill name
    if let Some(parent) = path.parent()
        && let Some(dir_name) = parent.file_name().and_then(|n| n.to_str())
        && dir_name != manifest.name
    {
        return Err(AivyxError::Config(format!(
            "SKILL.md name '{}' does not match directory name '{dir_name}'",
            manifest.name
        )));
    }

    Ok(SkillSummary {
        name: manifest.name,
        description: manifest.description,
        path: path.to_path_buf(),
    })
}

/// Result of comprehensive SKILL.md validation.
///
/// Separates blocking errors (must fix) from advisory warnings (best practices).
/// Use [`validate_full`] to produce a report from raw SKILL.md content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Errors that must be fixed (invalid name, missing required fields, etc.).
    pub errors: Vec<String>,
    /// Warnings for best-practice improvements (missing license, short description, etc.).
    pub warnings: Vec<String>,
}

impl ValidationReport {
    /// Returns `true` if there are no errors (warnings are allowed).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate a SKILL.md file comprehensively.
///
/// Goes beyond [`SkillManifest::validate()`] to check body structure, allowed-tools
/// syntax, and best-practice metadata. Returns a [`ValidationReport`] with errors
/// and warnings rather than failing on the first issue.
///
/// **Errors:**
/// - YAML frontmatter not parseable
/// - Missing or invalid `name` / `description`
/// - Body is empty (no content after frontmatter)
/// - Body contains no markdown heading (`# ...`)
/// - `allowed-tools` entries have invalid syntax
///
/// **Warnings:**
/// - Missing `license` field
/// - Missing `metadata.author`
/// - Missing `metadata.version`
/// - Description under 20 characters
pub fn validate_full(content: &str) -> ValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Try to parse frontmatter + body
    let parsed = SkillManifest::parse(content);
    let (manifest, body) = match parsed {
        Ok(pair) => pair,
        Err(e) => {
            errors.push(format!("parse error: {e}"));
            return ValidationReport { errors, warnings };
        }
    };

    // Name checks (already passed parse, but re-check for report)
    if manifest.name.is_empty() {
        errors.push("name is required".into());
    }

    // Description checks
    if manifest.description.is_empty() {
        errors.push("description is required".into());
    } else if manifest.description.len() < 20 {
        warnings.push("description is very short (under 20 characters)".into());
    }

    // Body checks
    if body.trim().is_empty() {
        errors.push("body is empty — add instructions after the frontmatter".into());
    } else if !body.lines().any(|line| line.starts_with('#')) {
        errors.push("body has no markdown headings — add at least one `# Heading`".into());
    }

    // Allowed-tools syntax check
    if let Some(ref tools_str) = manifest.allowed_tools {
        for token in tools_str.split_whitespace() {
            if !is_valid_tool_token(token) {
                errors.push(format!(
                    "invalid allowed-tools entry: '{token}' — expected 'ToolName' or 'ToolName(scope:pattern)'"
                ));
            }
        }
    }

    // Best-practice warnings
    if manifest.license.is_none() {
        warnings.push("missing 'license' field (recommended: SPDX identifier like 'MIT')".into());
    }
    if !manifest.metadata.contains_key("author") {
        warnings.push("missing 'metadata.author' field".into());
    }
    if !manifest.metadata.contains_key("version") {
        warnings.push("missing 'metadata.version' field".into());
    }

    ValidationReport { errors, warnings }
}

/// Check if a tool token matches the expected format.
///
/// Valid: `Read`, `Write`, `Bash`, `Bash(git:*)`, `Bash(npx:run-tests)`
/// Invalid: empty, `()`, `Bash(`, `(git:*)`
fn is_valid_tool_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    if let Some(paren_pos) = token.find('(') {
        // Must have content before '(' and end with ')'
        if paren_pos == 0 || !token.ends_with(')') {
            return false;
        }
        let name = &token[..paren_pos];
        let scope = &token[paren_pos + 1..token.len() - 1];
        // Name: letters and underscores
        name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !scope.is_empty()
            && scope
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '*' | '_' | '-' | '.'))
    } else {
        // Simple tool name: letters, digits, underscores
        token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_SKILL: &str = r#"---
name: webapp-testing
description: Guide for testing web applications using Playwright
license: MIT
compatibility: Requires Node.js >=18
allowed-tools: Bash(npx:*) Read Write
metadata:
  author: platform-team
  version: "1.0.0"
---
# Web Application Testing

## Overview
Use Playwright for browser automation.

## Workflow
1. Install browsers
2. Write tests
"#;

    const MINIMAL_SKILL: &str = r#"---
name: my-skill
description: A minimal skill
---
# Instructions here
"#;

    #[test]
    fn parse_full_skill() {
        let (manifest, body) = SkillManifest::parse(FULL_SKILL).unwrap();
        assert_eq!(manifest.name, "webapp-testing");
        assert_eq!(
            manifest.description,
            "Guide for testing web applications using Playwright"
        );
        assert_eq!(manifest.license.as_deref(), Some("MIT"));
        assert_eq!(
            manifest.compatibility.as_deref(),
            Some("Requires Node.js >=18")
        );
        assert_eq!(
            manifest.allowed_tools.as_deref(),
            Some("Bash(npx:*) Read Write")
        );
        assert_eq!(manifest.metadata.get("author").unwrap(), "platform-team");
        assert_eq!(manifest.metadata.get("version").unwrap(), "1.0.0");
        assert!(body.contains("# Web Application Testing"));
        assert!(body.contains("Playwright"));
    }

    #[test]
    fn parse_minimal_skill() {
        let (manifest, body) = SkillManifest::parse(MINIMAL_SKILL).unwrap();
        assert_eq!(manifest.name, "my-skill");
        assert_eq!(manifest.description, "A minimal skill");
        assert!(manifest.license.is_none());
        assert!(manifest.compatibility.is_none());
        assert!(manifest.allowed_tools.is_none());
        assert!(manifest.metadata.is_empty());
        assert!(body.contains("# Instructions here"));
    }

    #[test]
    fn reject_missing_frontmatter() {
        let result = SkillManifest::parse("# No frontmatter here");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("---"));
    }

    #[test]
    fn reject_missing_closing_delimiter() {
        let result = SkillManifest::parse("---\nname: test\ndescription: test\n# No closing");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("closing"));
    }

    #[test]
    fn reject_missing_name() {
        let content = "---\ndescription: A skill without a name\n---\n# Body";
        let result = SkillManifest::parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn reject_missing_description() {
        let content = "---\nname: no-desc\n---\n# Body";
        let result = SkillManifest::parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn reject_name_too_long() {
        let long_name = "a".repeat(65);
        let content = format!("---\nname: {long_name}\ndescription: Valid\n---\n# Body");
        let result = SkillManifest::parse(&content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("64"));
    }

    #[test]
    fn reject_name_invalid_chars() {
        let content = "---\nname: My_Skill\ndescription: Has uppercase and underscore\n---\n# Body";
        let result = SkillManifest::parse(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("lowercase"));
    }

    #[test]
    fn validate_name_allows_digits_and_hyphens() {
        let content = "---\nname: my-skill-2\ndescription: Valid name\n---\n# Body";
        let (manifest, _) = SkillManifest::parse(content).unwrap();
        assert_eq!(manifest.name, "my-skill-2");
    }

    #[test]
    fn serde_roundtrip() {
        let (manifest, _) = SkillManifest::parse(FULL_SKILL).unwrap();
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: SkillManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "webapp-testing");
        assert_eq!(parsed.license.as_deref(), Some("MIT"));
    }

    #[test]
    fn discover_skills_in_directory() {
        let dir = std::env::temp_dir().join(format!("aivyx-skills-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();

        // Create two valid skills
        let skill1_dir = dir.join("alpha-skill");
        std::fs::create_dir_all(&skill1_dir).unwrap();
        std::fs::write(
            skill1_dir.join("SKILL.md"),
            "---\nname: alpha-skill\ndescription: First skill\n---\n# Alpha",
        )
        .unwrap();

        let skill2_dir = dir.join("beta-skill");
        std::fs::create_dir_all(&skill2_dir).unwrap();
        std::fs::write(
            skill2_dir.join("SKILL.md"),
            "---\nname: beta-skill\ndescription: Second skill\n---\n# Beta",
        )
        .unwrap();

        // Create a hidden directory (should be skipped)
        let hidden_dir = dir.join(".hidden-skill");
        std::fs::create_dir_all(&hidden_dir).unwrap();
        std::fs::write(
            hidden_dir.join("SKILL.md"),
            "---\nname: hidden-skill\ndescription: Should be skipped\n---\n",
        )
        .unwrap();

        // Create a directory without SKILL.md (should be skipped)
        let no_skill_dir = dir.join("no-skill-here");
        std::fs::create_dir_all(&no_skill_dir).unwrap();

        let summaries = discover_skills(&dir).unwrap();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "alpha-skill");
        assert_eq!(summaries[1].name, "beta-skill");
        assert_eq!(summaries[0].description, "First skill");
        assert_eq!(summaries[1].description, "Second skill");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_skills_nonexistent_dir() {
        let dir = Path::new("/tmp/aivyx-nonexistent-skills-dir");
        let summaries = discover_skills(dir).unwrap();
        assert!(summaries.is_empty());
    }

    #[test]
    fn load_skill_full() {
        let dir = std::env::temp_dir().join(format!("aivyx-skill-load-{}", rand::random::<u64>()));
        let skill_dir = dir.join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, FULL_SKILL).unwrap();

        // Note: directory name doesn't match skill name for load_skill (no check)
        let loaded = load_skill(&skill_path).unwrap();
        assert_eq!(loaded.manifest.name, "webapp-testing");
        assert!(loaded.body.contains("Playwright"));
        assert_eq!(loaded.base_dir, skill_dir);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_rejects_name_mismatch() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-skills-mismatch-{}", rand::random::<u64>()));
        let skill_dir = dir.join("wrong-dir-name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: actual-name\ndescription: Name mismatch\n---\n# Body",
        )
        .unwrap();

        let summaries = discover_skills(&dir).unwrap();
        // Should skip the mismatched skill (logged as warning)
        assert!(summaries.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_multiline_description() {
        let content = r#"---
name: multi-line
description: >
  This is a long description that spans
  multiple lines using YAML folded style.
---
# Body
"#;
        let (manifest, _) = SkillManifest::parse(content).unwrap();
        assert!(manifest.description.contains("long description"));
        assert!(manifest.description.contains("multiple lines"));
    }

    #[test]
    fn parse_empty_body() {
        let content = "---\nname: empty-body\ndescription: No body content\n---\n";
        let (manifest, body) = SkillManifest::parse(content).unwrap();
        assert_eq!(manifest.name, "empty-body");
        assert!(body.is_empty());
    }

    // --- validate_full tests ---

    #[test]
    fn validate_full_valid_skill() {
        let report = validate_full(FULL_SKILL);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(
            report.warnings.is_empty(),
            "warnings: {:?}",
            report.warnings
        );
    }

    #[test]
    fn validate_full_minimal_skill_has_warnings() {
        let report = validate_full(MINIMAL_SKILL);
        assert!(report.is_ok()); // no errors
        assert!(report.warnings.iter().any(|w| w.contains("license")));
        assert!(report.warnings.iter().any(|w| w.contains("author")));
        assert!(report.warnings.iter().any(|w| w.contains("version")));
    }

    #[test]
    fn validate_full_empty_body_error() {
        let content = "---\nname: no-body\ndescription: A skill with no body content\n---\n";
        let report = validate_full(content);
        assert!(report.errors.iter().any(|e| e.contains("body is empty")));
    }

    #[test]
    fn validate_full_no_heading_error() {
        let content = "---\nname: no-heading\ndescription: A skill without headings\n---\nJust plain text without any heading.";
        let report = validate_full(content);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("no markdown headings"))
        );
    }

    #[test]
    fn validate_full_invalid_tools_syntax() {
        let content = "---\nname: bad-tools\ndescription: A skill with invalid tools syntax\nallowed-tools: Read (broken Write\n---\n# Body";
        let report = validate_full(content);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("invalid allowed-tools"))
        );
    }

    #[test]
    fn validate_full_short_description_warning() {
        let content = "---\nname: short-desc\ndescription: Short\nlicense: MIT\nmetadata:\n  author: me\n  version: \"1.0\"\n---\n# Body";
        let report = validate_full(content);
        assert!(report.is_ok());
        assert!(report.warnings.iter().any(|w| w.contains("very short")));
    }

    #[test]
    fn validate_full_parse_error() {
        let report = validate_full("not valid skill content");
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| e.contains("parse error")));
    }

    #[test]
    fn is_valid_tool_token_cases() {
        assert!(is_valid_tool_token("Read"));
        assert!(is_valid_tool_token("Write"));
        assert!(is_valid_tool_token("Bash"));
        assert!(is_valid_tool_token("Bash(git:*)"));
        assert!(is_valid_tool_token("Bash(npx:run-tests)"));
        assert!(!is_valid_tool_token(""));
        assert!(!is_valid_tool_token("(git:*)"));
        assert!(!is_valid_tool_token("Bash("));
        assert!(!is_valid_tool_token("Bash()"));
    }
}
