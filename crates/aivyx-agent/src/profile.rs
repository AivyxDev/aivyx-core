use std::path::Path;

use aivyx_config::McpServerConfig;
use aivyx_core::{AivyxError, AutonomyTier, CapabilityScope, Result};
use serde::{Deserialize, Serialize};

use crate::persona::Persona;

/// An agent's personality and configuration, loaded from TOML.
///
/// Profiles live at `~/.aivyx/agents/{name}.toml` and define the agent's
/// role, system prompt ("soul"), available tools, and autonomy overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Unique name for the agent (used as filename).
    pub name: String,
    /// Role description (e.g., "researcher", "coder").
    pub role: String,
    /// System prompt that shapes the agent's behavior.
    pub soul: String,
    /// Names of tools this agent is allowed to use.
    #[serde(default)]
    pub tool_ids: Vec<String>,
    /// Skills this agent can perform (informational, for prompting).
    #[serde(default)]
    pub skills: Vec<String>,
    /// Override the default autonomy tier from config.
    #[serde(default)]
    pub autonomy_tier: Option<AutonomyTier>,
    /// Named provider from the config's `[providers]` map.
    /// `None` uses the global `[provider]` config.
    #[serde(default)]
    pub provider: Option<String>,
    /// Ordered fallback provider names from config `[providers]`.
    /// Used when the primary provider's circuit breaker opens.
    /// Empty means no fallback (current behavior).
    #[serde(default)]
    pub fallback_providers: Vec<String>,
    /// Per-agent cache configuration override.
    /// `None` uses the global `[cache]` config.
    #[serde(default)]
    pub cache: Option<aivyx_config::CacheConfig>,
    /// Per-agent routing configuration override.
    /// `None` uses the global `[routing]` config.
    #[serde(default)]
    pub routing: Option<aivyx_config::RoutingConfig>,
    /// Maximum tokens for LLM responses.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Capabilities granted to this agent (scope + action pattern).
    #[serde(default)]
    pub capabilities: Vec<ProfileCapability>,
    /// MCP servers this agent connects to at creation time.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Structured personality. When present, [`effective_soul()`](Self::effective_soul)
    /// generates the system prompt from this instead of using the raw `soul` field.
    #[serde(default)]
    pub persona: Option<Persona>,
    /// Whether this agent can participate in the Nexus social network.
    ///
    /// Defaults to `true`. Set to `false` to prevent this agent from having
    /// Nexus tools (publish, browse, interact, etc.) even when the instance-level
    /// Nexus config is enabled. Useful for agents handling sensitive topics.
    #[serde(default = "default_nexus_enabled")]
    pub nexus_enabled: bool,
}

/// A capability entry in an agent profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileCapability {
    /// The scope domain for this capability.
    pub scope: CapabilityScope,
    /// Glob pattern for allowed actions (e.g., `"*"`, `"read:*"`).
    pub pattern: String,
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_nexus_enabled() -> bool {
    true
}

impl AgentProfile {
    /// Load a profile from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())?;
        toml::from_str(&content).map_err(|e| AivyxError::TomlDe(e.to_string()))
    }

    /// Save a profile to a TOML file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let content =
            toml::to_string_pretty(self).map_err(|e| AivyxError::TomlSer(e.to_string()))?;
        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }

    /// Return the effective system prompt for this profile.
    ///
    /// When both a [`Persona`] and a non-empty `soul` string are present,
    /// the custom soul is used as the identity foundation and persona
    /// behavioral guidelines are appended. This preserves user-crafted
    /// personality while layering on structured style rules.
    ///
    /// - **soul + persona** → `"{soul}\n\n{persona guidelines}"`
    /// - **persona only** (empty soul) → `persona.generate_soul(role)`
    /// - **soul only** (no persona) → raw `soul` string
    pub fn effective_soul(&self) -> String {
        match (&self.persona, self.soul.is_empty()) {
            (Some(persona), true) => persona.generate_soul(&self.role),
            (Some(persona), false) => {
                format!("{}\n\n{}", self.soul, persona.generate_guidelines())
            }
            _ => self.soul.clone(),
        }
    }

    /// Create a default template profile with the given name and role.
    /// Includes all built-in tools with full capabilities by default.
    pub fn template(name: &str, role: &str) -> Self {
        Self {
            name: name.to_string(),
            role: role.to_string(),
            soul: format!(
                "You are a helpful AI assistant acting as a {role}. \
                 Follow instructions carefully and be thorough in your work."
            ),
            tool_ids: vec![
                "file_read".into(),
                "file_write".into(),
                "shell".into(),
                "directory_list".into(),
                "grep_search".into(),
                "glob_find".into(),
                "system_time".into(),
            ],
            skills: Vec::new(),
            autonomy_tier: None,
            provider: None,
            fallback_providers: Vec::new(),
            cache: None,
            routing: None,
            max_tokens: default_max_tokens(),
            capabilities: default_capabilities(),
            mcp_servers: Vec::new(),
            persona: None,
            nexus_enabled: true,
        }
    }

    /// Create a specialized agent profile for a known role.
    ///
    /// Known roles: `assistant`, `coder`, `researcher`, `writer`, `ops`.
    /// Unknown roles fall back to the generic template.
    ///
    /// If `roles_dir` is provided, checks for `{roles_dir}/{role}.toml` first.
    /// User-defined role templates override the hardcoded presets.
    pub fn for_role(name: &str, role: &str) -> Self {
        Self::for_role_with_dir(name, role, None)
    }

    /// Like [`for_role`](Self::for_role), but accepts an optional roles directory
    /// to load user-defined role templates from.
    pub fn for_role_with_dir(name: &str, role: &str, roles_dir: Option<&Path>) -> Self {
        // Check for a user-defined role template first.
        if let Some(dir) = roles_dir {
            let role_path = dir.join(format!("{role}.toml"));
            if role_path.exists() {
                match Self::load(&role_path) {
                    Ok(mut profile) => {
                        // Override the name to match the requested agent name.
                        profile.name = name.to_string();
                        return profile;
                    }
                    Err(e) => {
                        eprintln!(
                            "  [warn] failed to load role template {}: {e}",
                            role_path.display()
                        );
                        // Fall through to hardcoded presets.
                    }
                }
            }
        }

        match role {
            "assistant" => Self::assistant_profile(name),
            "coder" => Self::coder_profile(name),
            "researcher" => Self::researcher_profile(name),
            "writer" => Self::writer_profile(name),
            "ops" => Self::ops_profile(name),
            _ => Self::template(name, role),
        }
    }

    /// Create a role profile with the given tools, skills, and optional overrides.
    ///
    /// Shared helper that eliminates boilerplate across role-specific constructors.
    /// All fields not specified use sensible defaults: empty soul (persona generates it),
    /// 8192 max tokens, default capabilities, and nexus enabled.
    fn role_profile(
        name: &str,
        role: &str,
        tool_ids: Vec<&str>,
        skills: Vec<&str>,
        autonomy_tier: Option<AutonomyTier>,
        capabilities: Option<Vec<ProfileCapability>>,
    ) -> Self {
        Self {
            name: name.to_string(),
            role: role.to_string(),
            soul: String::new(),
            tool_ids: tool_ids.into_iter().map(String::from).collect(),
            skills: skills.into_iter().map(String::from).collect(),
            autonomy_tier,
            provider: None,
            fallback_providers: Vec::new(),
            cache: None,
            routing: None,
            max_tokens: 8192,
            capabilities: capabilities.unwrap_or_else(default_capabilities),
            mcp_servers: Vec::new(),
            persona: Persona::for_role(role),
            nexus_enabled: true,
        }
    }

    fn assistant_profile(name: &str) -> Self {
        Self::role_profile(
            name,
            "assistant",
            vec![
                "file_read",
                "file_write",
                "shell",
                "web_search",
                "http_fetch",
                "project_tree",
                "project_outline",
                "file_delete",
                "file_move",
                "file_copy",
                "directory_list",
                "grep_search",
                "glob_find",
                "text_diff",
                "git_status",
                "git_diff",
                "git_log",
                "git_commit",
                "system_time",
                "json_parse",
            ],
            vec!["task management", "summarization", "scheduling", "research"],
            None,
            None,
        )
    }

    fn coder_profile(name: &str) -> Self {
        Self::role_profile(
            name,
            "coder",
            vec![
                "file_read",
                "file_write",
                "shell",
                "project_tree",
                "project_outline",
                "file_delete",
                "file_move",
                "file_copy",
                "directory_list",
                "grep_search",
                "glob_find",
                "text_diff",
                "git_status",
                "git_diff",
                "git_log",
                "git_commit",
                "system_time",
                "json_parse",
            ],
            vec![
                "code review",
                "debugging",
                "architecture",
                "testing",
                "refactoring",
            ],
            None,
            None,
        )
    }

    fn researcher_profile(name: &str) -> Self {
        Self::role_profile(
            name,
            "researcher",
            vec![
                "file_read",
                "file_write",
                "shell",
                "web_search",
                "http_fetch",
                "project_tree",
                "project_outline",
                "directory_list",
                "grep_search",
                "glob_find",
                "system_time",
                "json_parse",
            ],
            vec![
                "information synthesis",
                "literature review",
                "data analysis",
                "summarization",
            ],
            None,
            None,
        )
    }

    fn writer_profile(name: &str) -> Self {
        Self::role_profile(
            name,
            "writer",
            vec![
                "file_read",
                "file_write",
                "directory_list",
                "glob_find",
                "text_diff",
            ],
            vec![
                "technical writing",
                "editing",
                "documentation",
                "copywriting",
            ],
            None,
            Some(vec![
                ProfileCapability {
                    scope: CapabilityScope::Filesystem {
                        root: std::path::PathBuf::from("/"),
                    },
                    pattern: "*".to_string(),
                },
                ProfileCapability {
                    scope: CapabilityScope::Custom("memory".to_string()),
                    pattern: "*".to_string(),
                },
            ]),
        )
    }

    fn ops_profile(name: &str) -> Self {
        Self::role_profile(
            name,
            "ops",
            vec![
                "file_read",
                "file_write",
                "shell",
                "file_delete",
                "file_move",
                "file_copy",
                "directory_list",
                "grep_search",
                "glob_find",
                "git_status",
                "git_diff",
                "git_log",
                "system_time",
                "env_read",
                "hash_compute",
            ],
            vec![
                "system administration",
                "Docker",
                "CI/CD",
                "monitoring",
                "troubleshooting",
            ],
            Some(AutonomyTier::Leash),
            None,
        )
    }
}

/// Default capabilities: full filesystem, shell, and memory access.
fn default_capabilities() -> Vec<ProfileCapability> {
    vec![
        ProfileCapability {
            scope: CapabilityScope::Filesystem {
                root: std::path::PathBuf::from("/"),
            },
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Shell {
                allowed_commands: vec![],
            },
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Custom("memory".to_string()),
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            },
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Custom("mcp:*".to_string()),
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Custom("self-improvement".to_string()),
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Custom("plugin".to_string()),
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Custom("missions".to_string()),
            pattern: "*".to_string(),
        },
        ProfileCapability {
            scope: CapabilityScope::Custom("coordination".to_string()),
            pattern: "*".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_creates_valid_profile() {
        let profile = AgentProfile::template("test-agent", "researcher");
        assert_eq!(profile.name, "test-agent");
        assert_eq!(profile.role, "researcher");
        assert!(profile.soul.contains("researcher"));
        assert_eq!(profile.max_tokens, 4096);
    }

    #[test]
    fn save_load_roundtrip() {
        let profile = AgentProfile::template("roundtrip", "coder");
        let path =
            std::env::temp_dir().join(format!("aivyx-profile-test-{}.toml", uuid::Uuid::new_v4()));

        profile.save(&path).unwrap();
        let loaded = AgentProfile::load(&path).unwrap();

        assert_eq!(loaded.name, "roundtrip");
        assert_eq!(loaded.role, "coder");
        assert_eq!(loaded.max_tokens, 4096);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn toml_with_all_fields() {
        let toml_str = r#"
name = "researcher"
role = "researcher"
soul = "You research things."
tool_ids = ["file_read", "web_search"]
skills = ["summarization"]
autonomy_tier = "Trust"
max_tokens = 8192
"#;
        let profile: AgentProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.name, "researcher");
        assert_eq!(profile.tool_ids.len(), 2);
        assert_eq!(profile.autonomy_tier, Some(AutonomyTier::Trust));
        assert_eq!(profile.max_tokens, 8192);
    }

    #[test]
    fn toml_minimal() {
        let toml_str = r#"
name = "minimal"
role = "helper"
soul = "You help."
"#;
        let profile: AgentProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.name, "minimal");
        assert!(profile.tool_ids.is_empty());
        assert!(profile.autonomy_tier.is_none());
        assert_eq!(profile.max_tokens, 4096);
        assert!(profile.capabilities.is_empty());
    }

    #[test]
    fn profile_capability_serde_roundtrip() {
        let toml_str = r#"
name = "capper"
role = "coder"
soul = "You code."

[[capabilities]]
pattern = "*"

[capabilities.scope]
Filesystem = { root = "/home/user" }

[[capabilities]]
pattern = "read:*"

[capabilities.scope]
Shell = { allowed_commands = ["ls", "cat"] }
"#;
        let profile: AgentProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.capabilities.len(), 2);
        assert!(matches!(
            profile.capabilities[0].scope,
            CapabilityScope::Filesystem { .. }
        ));
        assert_eq!(profile.capabilities[0].pattern, "*");
        assert!(matches!(
            profile.capabilities[1].scope,
            CapabilityScope::Shell { .. }
        ));
    }

    #[test]
    fn toml_minimal_has_no_provider() {
        let toml_str = r#"
name = "minimal"
role = "helper"
soul = "You help."
"#;
        let profile: AgentProfile = toml::from_str(toml_str).unwrap();
        assert!(profile.provider.is_none());
    }

    #[test]
    fn toml_with_provider_field() {
        let toml_str = r#"
name = "researcher"
role = "researcher"
soul = "You research things."
provider = "reasoning"
tool_ids = ["file_read", "web_search"]
"#;
        let profile: AgentProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.provider.as_deref(), Some("reasoning"));
    }

    #[test]
    fn profile_with_persona_toml() {
        let toml_str = r#"
name = "custom"
role = "helper"
soul = ""

[persona]
formality = 0.8
verbosity = 0.3
warmth = 0.9
humor = 0.1
confidence = 0.6
curiosity = 0.5
tone = "warm and helpful"
uses_emoji = true
"#;
        let profile: AgentProfile = toml::from_str(toml_str).unwrap();
        assert!(profile.persona.is_some());
        let persona = profile.persona.unwrap();
        assert_eq!(persona.formality, 0.8);
        assert_eq!(persona.warmth, 0.9);
        assert!(persona.uses_emoji);
        assert_eq!(persona.tone.as_deref(), Some("warm and helpful"));
    }

    #[test]
    fn profile_without_persona_backward_compat() {
        let toml_str = r#"
name = "legacy"
role = "helper"
soul = "You are a helpful legacy agent."
"#;
        let profile: AgentProfile = toml::from_str(toml_str).unwrap();
        assert!(profile.persona.is_none());
        assert_eq!(profile.soul, "You are a helpful legacy agent.");
    }

    #[test]
    fn effective_soul_merges_soul_and_persona() {
        let mut profile = AgentProfile::template("test", "coder");
        profile.persona = Some(Persona::for_role("coder").unwrap());
        let soul = profile.effective_soul();
        // Custom soul identity is preserved
        assert!(soul.contains("helpful AI assistant acting as a coder"));
        // Persona guidelines are appended
        assert!(soul.contains("Do not use emoji"));
        // Role intro from generate_soul() is NOT present (we use guidelines)
        assert!(!soul.contains("You are an AI coder."));
    }

    #[test]
    fn effective_soul_empty_soul_uses_generate_soul() {
        let mut profile = AgentProfile::template("test", "coder");
        profile.soul = String::new();
        profile.persona = Some(Persona::for_role("coder").unwrap());
        let soul = profile.effective_soul();
        // Falls back to generate_soul() with role intro
        assert!(soul.contains("AI coder"));
    }

    #[test]
    fn effective_soul_no_persona_uses_raw_soul() {
        let profile = AgentProfile::template("test", "coder");
        assert!(profile.persona.is_none());
        let soul = profile.effective_soul();
        assert!(soul.contains("helpful AI assistant"));
    }

    #[test]
    fn for_role_with_dir_loads_custom_template() {
        let dir = std::env::temp_dir().join(format!("aivyx-roles-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let toml_content = r#"
name = "placeholder"
role = "custom-analyst"
soul = "You are a custom data analyst."
tool_ids = ["file_read", "json_parse"]
max_tokens = 16384
"#;
        std::fs::write(dir.join("custom-analyst.toml"), toml_content).unwrap();

        let profile = AgentProfile::for_role_with_dir("my-agent", "custom-analyst", Some(&dir));
        assert_eq!(profile.name, "my-agent"); // Name overridden
        assert_eq!(profile.role, "custom-analyst");
        assert!(profile.soul.contains("custom data analyst"));
        assert_eq!(profile.max_tokens, 16384);
        assert_eq!(profile.tool_ids, vec!["file_read", "json_parse"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn for_role_with_dir_falls_back_to_preset() {
        let dir = std::env::temp_dir().join(format!("aivyx-roles-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // No "coder.toml" in this directory — should use hardcoded preset.
        let profile = AgentProfile::for_role_with_dir("test", "coder", Some(&dir));
        assert_eq!(profile.role, "coder");
        assert!(profile.persona.is_some()); // Hardcoded preset has persona

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn role_profiles_have_personas() {
        for role in &["assistant", "coder", "researcher", "writer", "ops"] {
            let profile = AgentProfile::for_role("test", role);
            assert!(
                profile.persona.is_some(),
                "role profile '{role}' should have a persona"
            );
        }
    }
}
