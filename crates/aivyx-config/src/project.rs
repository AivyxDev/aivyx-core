//! Project registry configuration.
//!
//! Projects are directories that the user actively works in. Registering a
//! project allows the agent to scope memory recall, inject project context into
//! prompts, and navigate the codebase with awareness of the project's structure.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default glob patterns for directories to exclude from tree and outline
/// traversals. These are common build artifact and dependency directories.
fn default_exclude_patterns() -> Vec<String> {
    vec![
        ".git".into(),
        "target".into(),
        "node_modules".into(),
        "__pycache__".into(),
        ".venv".into(),
        "dist".into(),
        "build".into(),
        ".next".into(),
        ".svelte-kit".into(),
    ]
}

/// A registered project directory in the aivyx system.
///
/// Projects are stored as `[[projects]]` entries in `config.toml`. They are
/// non-secret metadata (paths and names), so they live in plain TOML rather
/// than in the encrypted store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Unique project name (slug-style, e.g., `"aivyx"`, `"my-webapp"`).
    pub name: String,
    /// Absolute filesystem path to the project root.
    pub path: PathBuf,
    /// Primary language or framework (e.g., `"Rust"`, `"TypeScript"`).
    #[serde(default)]
    pub language: Option<String>,
    /// Short description, often auto-generated from README.
    #[serde(default)]
    pub description: Option<String>,
    /// User-defined tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Directory names to exclude from tree and outline traversals.
    #[serde(default = "default_exclude_patterns")]
    pub exclude_patterns: Vec<String>,
    /// When this project was registered.
    pub registered_at: DateTime<Utc>,
    /// When this project was last used as the active context.
    #[serde(default)]
    pub last_accessed_at: Option<DateTime<Utc>>,
}

impl ProjectConfig {
    /// Create a new project config with defaults.
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            language: None,
            description: None,
            tags: Vec::new(),
            exclude_patterns: default_exclude_patterns(),
            registered_at: Utc::now(),
            last_accessed_at: None,
        }
    }

    /// Return the memory tag convention for this project: `"project:{name}"`.
    ///
    /// Memories tagged with this value are scoped to this project and will be
    /// preferentially recalled when the project is active.
    pub fn project_tag(&self) -> String {
        format!("project:{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_with_defaults() {
        let p = ProjectConfig::new("aivyx", "/home/user/Projects/aivyx");
        assert_eq!(p.name, "aivyx");
        assert_eq!(p.path, PathBuf::from("/home/user/Projects/aivyx"));
        assert!(p.language.is_none());
        assert!(p.description.is_none());
        assert!(p.tags.is_empty());
        assert!(!p.exclude_patterns.is_empty());
        assert!(p.exclude_patterns.contains(&"target".to_string()));
        assert!(p.last_accessed_at.is_none());
    }

    #[test]
    fn project_tag_format() {
        let p = ProjectConfig::new("my-webapp", "/tmp/webapp");
        assert_eq!(p.project_tag(), "project:my-webapp");
    }

    #[test]
    fn serde_roundtrip() {
        let mut p = ProjectConfig::new("aivyx", "/home/user/aivyx");
        p.language = Some("Rust".into());
        p.description = Some("AI agent framework".into());
        p.tags = vec!["ai".into(), "rust".into()];

        let json = serde_json::to_string(&p).unwrap();
        let parsed: ProjectConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, "aivyx");
        assert_eq!(parsed.language.as_deref(), Some("Rust"));
        assert_eq!(parsed.description.as_deref(), Some("AI agent framework"));
        assert_eq!(parsed.tags, vec!["ai", "rust"]);
        assert_eq!(parsed.exclude_patterns, p.exclude_patterns);
    }

    #[test]
    fn serde_partial_fields_ok() {
        let json = r#"{"name":"test","path":"/tmp/test","registered_at":"2026-01-01T00:00:00Z"}"#;
        let p: ProjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(p.name, "test");
        assert!(p.language.is_none());
        assert_eq!(p.exclude_patterns, default_exclude_patterns());
    }

    #[test]
    fn toml_roundtrip() {
        let mut p = ProjectConfig::new("aivyx", "/home/user/aivyx");
        p.language = Some("Rust".into());

        let toml_str = toml::to_string(&p).unwrap();
        let parsed: ProjectConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.name, "aivyx");
        assert_eq!(parsed.language.as_deref(), Some("Rust"));
    }
}
