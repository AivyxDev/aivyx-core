//! Self-improvement tools for agent self-reflection and configuration.
//!
//! [`SelfProfileTool`] lets an agent read its own profile.
//! [`SelfUpdateTool`] lets an agent modify safe profile fields.
//!
//! Security-critical fields (name, autonomy_tier, capabilities, mcp_servers,
//! provider) are forbidden — only the user can change these.

use async_trait::async_trait;
use tracing::warn;

use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_config::AivyxDirs;
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};

use crate::profile::AgentProfile;

/// Fields that agents are allowed to modify on themselves.
const ALLOWED_FIELDS: &[&str] = &[
    "role",
    "soul",
    "skills",
    "tool_ids",
    "max_tokens",
    "persona.tone",
    "persona.formality",
    "persona.verbosity",
    "persona.warmth",
    "persona.humor",
    "persona.confidence",
    "persona.curiosity",
    "persona.uses_emoji",
];

/// Fields that are forbidden from self-modification for security.
const FORBIDDEN_FIELDS: &[&str] = &[
    "name",
    "autonomy_tier",
    "capabilities",
    "mcp_servers",
    "provider",
];

// ---------------------------------------------------------------------------
// SelfProfileTool
// ---------------------------------------------------------------------------

/// Tool that lets an agent read its own profile configuration.
pub struct SelfProfileTool {
    id: ToolId,
    dirs: AivyxDirs,
    agent_name: String,
}

impl SelfProfileTool {
    /// Create a new self-profile reading tool.
    pub fn new(dirs: AivyxDirs, agent_name: String) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
            agent_name,
        }
    }
}

#[async_trait]
impl Tool for SelfProfileTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "self_profile"
    }

    fn description(&self) -> &str {
        "Read your own agent profile configuration including role, soul, tools, persona, and skills."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("self-improvement".into()))
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let profile_path = self
            .dirs
            .agents_dir()
            .join(format!("{}.toml", self.agent_name));

        if !profile_path.exists() {
            return Err(AivyxError::Config(format!(
                "own profile not found: {} (expected at {})",
                self.agent_name,
                profile_path.display()
            )));
        }

        let profile = AgentProfile::load(&profile_path)?;
        serde_json::to_value(&profile).map_err(|e| AivyxError::Agent(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// SelfUpdateTool
// ---------------------------------------------------------------------------

/// Tool that lets an agent modify safe fields in its own profile.
///
/// Forbidden fields: `name`, `autonomy_tier`, `capabilities`, `mcp_servers`, `provider`.
/// These require explicit user action to change.
pub struct SelfUpdateTool {
    id: ToolId,
    dirs: AivyxDirs,
    agent_name: String,
    audit_log: Option<AuditLog>,
}

impl SelfUpdateTool {
    /// Create a new self-update tool.
    pub fn new(dirs: AivyxDirs, agent_name: String, audit_log: Option<AuditLog>) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
            agent_name,
            audit_log,
        }
    }

    fn audit(&self, event: AuditEvent) {
        if let Some(log) = &self.audit_log
            && let Err(e) = log.append(event)
        {
            warn!("Failed to write audit event: {e}");
        }
    }
}

#[async_trait]
impl Tool for SelfUpdateTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "self_update"
    }

    fn description(&self) -> &str {
        "Modify a field in your own agent profile. Allowed: role, soul, skills, tool_ids, max_tokens, persona.* fields. Forbidden: name, autonomy_tier, capabilities, mcp_servers, provider."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "field": {
                    "type": "string",
                    "description": "Profile field to modify (e.g., 'role', 'soul', 'max_tokens', 'persona.tone')"
                },
                "value": {
                    "description": "New value for the field. Type depends on field: string for role/soul/persona.tone, number for max_tokens/persona.formality, array of strings for skills/tool_ids, boolean for persona.uses_emoji."
                }
            },
            "required": ["field", "value"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("self-improvement".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let field = input["field"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("self_update: missing 'field'".into()))?;
        let value = &input["value"];

        if value.is_null() {
            return Err(AivyxError::Agent("self_update: missing 'value'".into()));
        }

        // Security check: reject forbidden fields
        if FORBIDDEN_FIELDS.contains(&field) {
            return Err(AivyxError::Agent(format!(
                "self_update: field '{field}' cannot be modified by the agent (security restriction)"
            )));
        }

        // Validate field is in allowed list
        if !ALLOWED_FIELDS.contains(&field) {
            return Err(AivyxError::Agent(format!(
                "self_update: unknown field '{field}'. Allowed: {}",
                ALLOWED_FIELDS.join(", ")
            )));
        }

        let profile_path = self
            .dirs
            .agents_dir()
            .join(format!("{}.toml", self.agent_name));

        if !profile_path.exists() {
            return Err(AivyxError::Config(format!(
                "own profile not found: {}",
                self.agent_name
            )));
        }

        let mut profile = AgentProfile::load(&profile_path)?;

        // Apply the field change
        apply_field_change(&mut profile, field, value)?;

        profile.save(&profile_path)?;

        self.audit(AuditEvent::SelfProfileModified {
            agent_name: self.agent_name.clone(),
            fields_changed: vec![field.to_string()],
        });

        Ok(serde_json::json!({
            "status": "updated",
            "field": field,
            "agent": self.agent_name
        }))
    }
}

/// Apply a field change to an agent profile.
fn apply_field_change(
    profile: &mut AgentProfile,
    field: &str,
    value: &serde_json::Value,
) -> Result<()> {
    match field {
        "role" => {
            profile.role = value
                .as_str()
                .ok_or_else(|| AivyxError::Agent("'role' must be a string".into()))?
                .to_string();
        }
        "soul" => {
            profile.soul = value
                .as_str()
                .ok_or_else(|| AivyxError::Agent("'soul' must be a string".into()))?
                .to_string();
        }
        "skills" => {
            let arr = value
                .as_array()
                .ok_or_else(|| AivyxError::Agent("'skills' must be an array of strings".into()))?;
            profile.skills = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        "tool_ids" => {
            let arr = value.as_array().ok_or_else(|| {
                AivyxError::Agent("'tool_ids' must be an array of strings".into())
            })?;
            profile.tool_ids = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        "max_tokens" => {
            let n = value.as_u64().ok_or_else(|| {
                AivyxError::Agent("'max_tokens' must be a positive integer".into())
            })?;
            profile.max_tokens = n.min(u32::MAX as u64) as u32;
        }
        f if f.starts_with("persona.") => {
            let persona = profile.persona.get_or_insert_with(Default::default);
            let sub_field = &f["persona.".len()..];
            match sub_field {
                "tone" => {
                    persona.tone = Some(
                        value
                            .as_str()
                            .ok_or_else(|| {
                                AivyxError::Agent("'persona.tone' must be a string".into())
                            })?
                            .to_string(),
                    );
                }
                "formality" => {
                    persona.formality = value.as_f64().ok_or_else(|| {
                        AivyxError::Agent("'persona.formality' must be a number".into())
                    })? as f32;
                }
                "verbosity" => {
                    persona.verbosity = value.as_f64().ok_or_else(|| {
                        AivyxError::Agent("'persona.verbosity' must be a number".into())
                    })? as f32;
                }
                "warmth" => {
                    persona.warmth = value.as_f64().ok_or_else(|| {
                        AivyxError::Agent("'persona.warmth' must be a number".into())
                    })? as f32;
                }
                "humor" => {
                    persona.humor = value.as_f64().ok_or_else(|| {
                        AivyxError::Agent("'persona.humor' must be a number".into())
                    })? as f32;
                }
                "confidence" => {
                    persona.confidence = value.as_f64().ok_or_else(|| {
                        AivyxError::Agent("'persona.confidence' must be a number".into())
                    })? as f32;
                }
                "curiosity" => {
                    persona.curiosity = value.as_f64().ok_or_else(|| {
                        AivyxError::Agent("'persona.curiosity' must be a number".into())
                    })? as f32;
                }
                "uses_emoji" => {
                    persona.uses_emoji = value.as_bool().ok_or_else(|| {
                        AivyxError::Agent("'persona.uses_emoji' must be a boolean".into())
                    })?;
                }
                _ => {
                    return Err(AivyxError::Agent(format!(
                        "unknown persona field: {sub_field}"
                    )));
                }
            }
        }
        _ => {
            return Err(AivyxError::Agent(format!("unhandled field: {field}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_profile_tool_schema() {
        let dirs = AivyxDirs::new("/tmp/test-aivyx");
        let tool = SelfProfileTool::new(dirs, "test-agent".into());
        assert_eq!(tool.name(), "self_profile");
        let schema = tool.input_schema();
        assert!(schema["properties"].is_object());
    }

    #[test]
    fn self_update_tool_schema() {
        let dirs = AivyxDirs::new("/tmp/test-aivyx");
        let tool = SelfUpdateTool::new(dirs, "test-agent".into(), None);
        assert_eq!(tool.name(), "self_update");
        let schema = tool.input_schema();
        assert!(schema["properties"]["field"].is_object());
        assert!(schema["properties"]["value"].is_object());
    }

    #[test]
    fn self_update_forbidden_fields() {
        let mut profile = AgentProfile::template("test", "coder");

        // Forbidden fields should error
        for field in FORBIDDEN_FIELDS {
            let result = apply_field_change(&mut profile, field, &serde_json::json!("anything"));
            assert!(result.is_err(), "field '{field}' should be forbidden");
        }
    }

    #[test]
    fn self_update_allowed_fields() {
        let mut profile = AgentProfile::template("test", "coder");

        // role
        apply_field_change(&mut profile, "role", &serde_json::json!("researcher")).unwrap();
        assert_eq!(profile.role, "researcher");

        // soul
        apply_field_change(&mut profile, "soul", &serde_json::json!("New soul text")).unwrap();
        assert_eq!(profile.soul, "New soul text");

        // max_tokens
        apply_field_change(&mut profile, "max_tokens", &serde_json::json!(16384)).unwrap();
        assert_eq!(profile.max_tokens, 16384);

        // skills
        apply_field_change(
            &mut profile,
            "skills",
            &serde_json::json!(["coding", "testing"]),
        )
        .unwrap();
        assert_eq!(profile.skills, vec!["coding", "testing"]);

        // persona.warmth
        apply_field_change(&mut profile, "persona.warmth", &serde_json::json!(0.9)).unwrap();
        assert!((profile.persona.as_ref().unwrap().warmth - 0.9).abs() < f32::EPSILON);
    }
}
