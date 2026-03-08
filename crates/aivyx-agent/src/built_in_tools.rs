use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;

/// Maximum output length in characters for tool results (file reads, diffs, etc.).
pub const MAX_TOOL_OUTPUT_CHARS: usize = 8000;

/// Check if an IP address is in a private, loopback, or link-local range.
fn is_private_or_loopback(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()       // 127.0.0.0/8
            || v4.is_private()     // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
            || v4.is_link_local()  // 169.254.0.0/16
            || v4.is_unspecified() // 0.0.0.0
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()       // ::1
            || v6.is_unspecified() // ::
            || (v6.segments()[0] & 0xff00) == 0xfd00 // fd00::/8 unique local
        }
    }
}

/// Validate a URL for fetching: only http/https schemes, no private IPs.
pub fn validate_fetch_url(url_str: &str) -> Result<String> {
    // Check scheme
    if !url_str.starts_with("http://") && !url_str.starts_with("https://") {
        return Err(AivyxError::Agent(format!(
            "http_fetch: unsupported URL scheme (only http/https allowed): {url_str}"
        )));
    }

    // Extract host from URL
    let after_scheme = if let Some(rest) = url_str.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url_str.strip_prefix("http://") {
        rest
    } else {
        return Err(AivyxError::Agent("http_fetch: invalid URL".into()));
    };

    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    let host = if host_port.starts_with('[') {
        // IPv6 literal: [::1]:port
        host_port
            .split(']')
            .next()
            .unwrap_or(host_port)
            .trim_start_matches('[')
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };

    if host.is_empty() {
        return Err(AivyxError::Agent("http_fetch: URL has no host".into()));
    }

    // Resolve DNS and check all addresses
    use std::net::ToSocketAddrs;
    let port: u16 = if url_str.starts_with("https://") {
        443
    } else {
        80
    };
    let addrs: Vec<std::net::SocketAddr> = format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| {
            AivyxError::Agent(format!(
                "http_fetch: DNS resolution failed for '{host}': {e}"
            ))
        })?
        .collect();

    if addrs.is_empty() {
        return Err(AivyxError::Agent(format!(
            "http_fetch: no DNS results for '{host}'"
        )));
    }

    for addr in &addrs {
        if is_private_or_loopback(&addr.ip()) {
            return Err(AivyxError::Agent(format!(
                "http_fetch: refusing to fetch private/loopback address {}",
                addr.ip()
            )));
        }
    }

    Ok(url_str.to_string())
}

/// Dangerous system paths that should never be targets of file operations.
const DANGEROUS_PATHS: &[&str] = &[
    "/", "/home", "/etc", "/usr", "/var", "/boot", "/root", "/tmp", "/bin", "/sbin", "/lib",
    "/lib64", "/dev", "/proc", "/sys",
];

/// Resolve a filesystem path by canonicalizing it and rejecting dangerous system paths.
///
/// Used by file-operation tools to prevent writes to `/`, `/etc`, etc.
pub async fn resolve_and_validate_path(
    path_str: &str,
    tool_name: &str,
) -> Result<std::path::PathBuf> {
    let path = std::path::Path::new(path_str);

    // For existing paths, canonicalize to resolve symlinks and normalize
    let canonical = if path.exists() {
        tokio::fs::canonicalize(path).await.map_err(|e| {
            AivyxError::Agent(format!(
                "{tool_name}: cannot resolve path '{path_str}': {e}"
            ))
        })?
    } else {
        // For new files, canonicalize the parent directory
        let parent = path.parent().ok_or_else(|| {
            AivyxError::Agent(format!(
                "{tool_name}: path '{path_str}' has no parent directory"
            ))
        })?;
        let canonical_parent = tokio::fs::canonicalize(parent).await.map_err(|e| {
            AivyxError::Agent(format!(
                "{tool_name}: cannot resolve parent of '{path_str}': {e}"
            ))
        })?;
        let file_name = path.file_name().ok_or_else(|| {
            AivyxError::Agent(format!("{tool_name}: path '{path_str}' has no file name"))
        })?;
        canonical_parent.join(file_name)
    };

    // Check if the canonical path matches any dangerous path
    for dangerous in DANGEROUS_PATHS {
        let dp = std::path::Path::new(dangerous);
        if canonical == dp {
            return Err(AivyxError::Agent(format!(
                "{tool_name}: refusing to operate on dangerous path '{}'",
                canonical.display()
            )));
        }
    }

    Ok(canonical)
}

/// Built-in tool: execute a shell command.
pub struct ShellTool {
    id: ToolId,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                }
            },
            "required": ["command"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Shell {
            allowed_commands: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let command = input["command"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("shell: missing 'command' parameter".into()))?;

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
            .map_err(AivyxError::Io)?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(serde_json::json!({
            "exit_code": output.status.code().unwrap_or(-1),
            "stdout": stdout,
            "stderr": stderr,
        }))
    }
}

/// Built-in tool: activate a SKILL.md skill by name (Tier 2 load).
///
/// Returns the full skill body as a tool result so the agent can follow
/// the skill's instructions in subsequent turns.
pub struct SkillActivateTool {
    id: ToolId,
    loader: std::sync::Arc<tokio::sync::Mutex<crate::skill_loader::SkillLoader>>,
}

impl SkillActivateTool {
    /// Create a new `SkillActivateTool` with a shared skill loader.
    pub fn new(
        loader: std::sync::Arc<tokio::sync::Mutex<crate::skill_loader::SkillLoader>>,
    ) -> Self {
        Self {
            id: ToolId::new(),
            loader,
        }
    }
}

#[async_trait]
impl Tool for SkillActivateTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "skill_activate"
    }

    fn description(&self) -> &str {
        "Activate an available skill by name to load its detailed instructions. \
         Use this when a skill listed in [AVAILABLE SKILLS] is relevant to the current task."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill name to activate (from [AVAILABLE SKILLS])"
                }
            },
            "required": ["name"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        // Skills are informational context — no capability check needed.
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("skill_activate: missing 'name' parameter".into()))?;

        let mut loader = self.loader.lock().await;
        let skill = loader.activate(name)?;

        Ok(serde_json::json!({
            "skill": name,
            "instructions": skill.body,
            "compatibility": skill.manifest.compatibility,
            "allowed_tools": skill.manifest.allowed_tools,
        }))
    }
}

/// Register all built-in tools into a ToolRegistry, filtered by the allowed tool names.
pub fn register_built_in_tools(registry: &mut aivyx_core::ToolRegistry, allowed_names: &[String]) {
    let mut all_tools: Vec<Box<dyn Tool>> = vec![Box::new(ShellTool::new())];

    // Filesystem tools: file_read, file_write, file_delete, file_move, file_copy, directory_list
    all_tools.extend(crate::filesystem_tools::create_filesystem_tools());

    // Search tools: project_tree, project_outline, grep_search, glob_find
    all_tools.extend(crate::search_tools::create_search_tools());

    // VCS tools: git_status, git_diff, git_log, git_commit
    all_tools.extend(crate::vcs_tools::create_vcs_tools());

    // Data tools: system_time, env_read, json_parse, hash_compute
    all_tools.extend(crate::data_tools::create_data_tools());

    // Web tools: web_search, http_fetch, text_diff
    all_tools.extend(crate::web_tools::create_web_tools());

    // Analysis & computation tools
    all_tools.extend(crate::analysis_tools::create_analysis_tools());

    // Document intelligence tools
    #[cfg(feature = "document-tools")]
    all_tools.extend(crate::document_tools::create_document_tools());

    // Network & communication tools (stateless only; contextual tools in session.rs)
    #[cfg(feature = "network-tools")]
    all_tools.extend(crate::network_tools::create_network_tools());

    // Infrastructure, safety & advanced tools (stateless only; schedule_task in session.rs)
    #[cfg(feature = "infrastructure-tools")]
    all_tools.extend(crate::infrastructure_tools::create_infrastructure_tools());

    for tool in all_tools {
        if allowed_names.is_empty() || allowed_names.iter().any(|n| n == tool.name()) {
            registry.register(tool);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_schema() {
        let tool = ShellTool::new();
        assert_eq!(tool.name(), "shell");
        let schema = tool.input_schema();
        assert!(schema["properties"]["command"].is_object());
    }

    #[test]
    fn shell_required_scope() {
        let tool = ShellTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Shell { .. }));
    }

    #[test]
    fn register_filters_by_name() {
        let mut registry = aivyx_core::ToolRegistry::new();
        register_built_in_tools(&mut registry, &["file_read".into()]);
        assert_eq!(registry.list().len(), 1);
        assert!(registry.get_by_name("file_read").is_some());
        assert!(registry.get_by_name("shell").is_none());
    }

    #[test]
    fn register_all_when_empty_filter() {
        let mut registry = aivyx_core::ToolRegistry::new();
        register_built_in_tools(&mut registry, &[]);
        assert_eq!(registry.list().len(), 46);
    }

    #[tokio::test]
    async fn skill_activate_valid_name() {
        let dir = std::env::temp_dir().join(format!("aivyx-sa-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let skill_dir = dir.join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test\ncompatibility: Rust 1.90+\n---\n# Instructions\n\nDo the thing.",
        ).unwrap();

        let loader = crate::skill_loader::SkillLoader::discover(&[dir.clone()]).unwrap();
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(loader));
        let tool = SkillActivateTool::new(arc);

        let result = tool
            .execute(serde_json::json!({ "name": "test-skill" }))
            .await
            .unwrap();

        assert_eq!(result["skill"], "test-skill");
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("Do the thing")
        );
        assert_eq!(result["compatibility"], "Rust 1.90+");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn skill_activate_unknown_name() {
        let dir = std::env::temp_dir().join(format!("aivyx-sa-unk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let loader = crate::skill_loader::SkillLoader::discover(&[dir.clone()]).unwrap();
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(loader));
        let tool = SkillActivateTool::new(arc);

        let result = tool
            .execute(serde_json::json!({ "name": "nonexistent" }))
            .await;
        assert!(result.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skill_activate_schema() {
        let loader = crate::skill_loader::SkillLoader::discover(&[]).unwrap();
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(loader));
        let tool = SkillActivateTool::new(arc);

        assert_eq!(tool.name(), "skill_activate");
        assert!(tool.required_scope().is_none());
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["name"].is_object());
    }
}
