//! Version control tools: git status, diff, log, and commit.

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;

use crate::built_in_tools::MAX_TOOL_OUTPUT_CHARS;

/// Built-in tool: show git working tree status including staged, modified, and untracked files.
pub struct GitStatusTool {
    id: ToolId,
}

impl Default for GitStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitStatusTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show git working tree status including staged, modified, and untracked files."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Repository path (defaults to current directory)"
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Shell {
            allowed_commands: vec!["git".into()],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let mut cmd = tokio::process::Command::new("git");
        if let Some(path) = input["path"].as_str() {
            cmd.arg("-C").arg(path);
        }
        cmd.args(["status", "--porcelain", "-b"]);

        let output = cmd.output().await.map_err(AivyxError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(AivyxError::Agent(format!("git_status: {stderr}")));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        let mut branch = String::new();
        let mut staged = Vec::new();
        let mut modified = Vec::new();
        let mut untracked = Vec::new();

        for line in stdout.lines() {
            if let Some(branch_info) = line.strip_prefix("## ") {
                branch = branch_info.to_string();
                continue;
            }

            if line.len() < 2 {
                continue;
            }

            let index_col = line.as_bytes().first().copied().unwrap_or(b' ');
            let worktree_col = line.as_bytes().get(1).copied().unwrap_or(b' ');
            let filename = line.get(3..).unwrap_or("").to_string();

            if matches!(index_col, b'A' | b'M' | b'D' | b'R' | b'C') {
                staged.push(filename.clone());
            }

            if matches!(worktree_col, b'M' | b'D') {
                modified.push(filename.clone());
            }

            if index_col == b'?' && worktree_col == b'?' {
                untracked.push(filename);
            }
        }

        let clean = staged.is_empty() && modified.is_empty() && untracked.is_empty();

        Ok(serde_json::json!({
            "branch": branch,
            "staged": staged,
            "modified": modified,
            "untracked": untracked,
            "clean": clean,
        }))
    }
}

/// Built-in tool: show file changes in a git repository.
pub struct GitDiffTool {
    id: ToolId,
}

impl Default for GitDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitDiffTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for GitDiffTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show file changes in a git repository."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Repository path (defaults to current directory)"
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show staged changes (default: false)"
                },
                "file": {
                    "type": "string",
                    "description": "Specific file to diff"
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Shell {
            allowed_commands: vec!["git".into()],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo_path = input["path"].as_str();
        let staged = input["staged"].as_bool().unwrap_or(false);
        let file = input["file"].as_str();

        let mut cmd = tokio::process::Command::new("git");
        if let Some(path) = repo_path {
            cmd.arg("-C").arg(path);
        }
        cmd.arg("diff");
        if staged {
            cmd.arg("--staged");
        }
        if let Some(f) = file {
            cmd.arg("--").arg(f);
        }

        let output = cmd.output().await.map_err(AivyxError::Io)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(AivyxError::Agent(format!("git_diff: {stderr}")));
        }

        let diff = String::from_utf8_lossy(&output.stdout).to_string();

        // Run git diff --stat for summary.
        let mut stat_cmd = tokio::process::Command::new("git");
        if let Some(path) = repo_path {
            stat_cmd.arg("-C").arg(path);
        }
        stat_cmd.arg("diff").arg("--stat");
        if staged {
            stat_cmd.arg("--staged");
        }
        if let Some(f) = file {
            stat_cmd.arg("--").arg(f);
        }

        let stat_output = stat_cmd.output().await.map_err(AivyxError::Io)?;
        let stat_text = String::from_utf8_lossy(&stat_output.stdout).to_string();

        let mut files_changed: u64 = 0;
        let mut insertions: u64 = 0;
        let mut deletions: u64 = 0;

        if let Some(last_line) = stat_text.lines().last() {
            for part in last_line.split(',') {
                let trimmed = part.trim();
                if trimmed.contains("file") {
                    files_changed = trimmed
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                } else if trimmed.contains("insertion") {
                    insertions = trimmed
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                } else if trimmed.contains("deletion") {
                    deletions = trimmed
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                }
            }
        }

        // Truncate diff if too long.
        let max_len = MAX_TOOL_OUTPUT_CHARS;
        let truncated_diff = if diff.len() > max_len {
            let boundary = diff.floor_char_boundary(max_len);
            format!("{}... [truncated]", &diff[..boundary])
        } else {
            diff
        };

        Ok(serde_json::json!({
            "diff": truncated_diff,
            "files_changed": files_changed,
            "insertions": insertions,
            "deletions": deletions,
        }))
    }
}

/// Built-in tool: view commit history of a git repository.
pub struct GitLogTool {
    id: ToolId,
}

impl Default for GitLogTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitLogTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for GitLogTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "View commit history of a git repository."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Repository path (defaults to current directory)"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (default: 10)"
                },
                "oneline": {
                    "type": "boolean",
                    "description": "Use oneline format (default: false)"
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Shell {
            allowed_commands: vec!["git".into()],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let count = input["count"].as_u64().unwrap_or(10);
        let oneline = input["oneline"].as_bool().unwrap_or(false);
        let count_str = count.to_string();

        let mut cmd = tokio::process::Command::new("git");
        if let Some(path) = input["path"].as_str() {
            cmd.arg("-C").arg(path);
        }
        cmd.arg("log");

        if oneline {
            cmd.args(["--oneline", "-n", &count_str]);
        } else {
            cmd.args(["--format=%H%n%an%n%aI%n%s", "-n", &count_str]);
        }

        let output = cmd.output().await.map_err(AivyxError::Io)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(AivyxError::Agent(format!("git_log: {stderr}")));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        let commits: Vec<serde_json::Value> = if oneline {
            stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|line| {
                    let (hash, message) = line.split_once(' ').unwrap_or((line, ""));
                    serde_json::json!({
                        "hash": hash,
                        "message": message,
                    })
                })
                .collect()
        } else {
            let lines: Vec<&str> = stdout.lines().collect();
            lines
                .chunks(4)
                .filter(|chunk| chunk.len() == 4)
                .map(|chunk| {
                    serde_json::json!({
                        "hash": chunk[0],
                        "author": chunk[1],
                        "date": chunk[2],
                        "message": chunk[3],
                    })
                })
                .collect()
        };

        Ok(serde_json::json!({
            "commits": commits,
        }))
    }
}

/// Built-in tool: stage specific files and create a git commit.
pub struct GitCommitTool {
    id: ToolId,
}

impl Default for GitCommitTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitCommitTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for GitCommitTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Stage specific files and create a git commit."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files to stage (must not be empty)"
                },
                "message": {
                    "type": "string",
                    "description": "Commit message"
                },
                "path": {
                    "type": "string",
                    "description": "Repository path (defaults to current directory)"
                }
            },
            "required": ["files", "message"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Shell {
            allowed_commands: vec!["git".into()],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let files = input["files"]
            .as_array()
            .ok_or_else(|| AivyxError::Agent("git_commit: missing 'files' parameter".into()))?;

        if files.is_empty() {
            return Err(AivyxError::Agent(
                "git_commit: 'files' array must not be empty".into(),
            ));
        }

        let file_paths: Vec<&str> = files
            .iter()
            .map(|f| {
                f.as_str().ok_or_else(|| {
                    AivyxError::Agent("git_commit: each file must be a string".into())
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let message = input["message"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("git_commit: missing 'message' parameter".into()))?;

        if message.is_empty() {
            return Err(AivyxError::Agent(
                "git_commit: 'message' must not be empty".into(),
            ));
        }

        let repo_path = input["path"].as_str();

        // Stage files.
        let mut add_cmd = tokio::process::Command::new("git");
        if let Some(path) = repo_path {
            add_cmd.arg("-C").arg(path);
        }
        add_cmd.arg("add").arg("--");
        for f in &file_paths {
            add_cmd.arg(f);
        }

        let add_output = add_cmd.output().await.map_err(AivyxError::Io)?;
        if !add_output.status.success() {
            let stderr = String::from_utf8_lossy(&add_output.stderr).to_string();
            return Err(AivyxError::Agent(format!(
                "git_commit: git add failed: {stderr}"
            )));
        }

        // Create commit.
        let mut commit_cmd = tokio::process::Command::new("git");
        if let Some(path) = repo_path {
            commit_cmd.arg("-C").arg(path);
        }
        commit_cmd.args(["commit", "-m", message]);

        let commit_output = commit_cmd.output().await.map_err(AivyxError::Io)?;
        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr).to_string();
            return Err(AivyxError::Agent(format!(
                "git_commit: git commit failed: {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&commit_output.stdout).to_string();

        // Parse commit hash from output.
        let hash = stdout
            .split_whitespace()
            .find(|word| word.len() >= 7 && word.chars().all(|c| c.is_ascii_hexdigit()))
            .or_else(|| {
                stdout
                    .split(']')
                    .next()
                    .and_then(|s| s.split_whitespace().last())
            })
            .unwrap_or("unknown")
            .trim_end_matches(']')
            .to_string();

        Ok(serde_json::json!({
            "status": "committed",
            "hash": hash,
            "files_committed": file_paths.len(),
            "message": message,
        }))
    }
}

/// Create all VCS tools.
pub fn create_vcs_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(GitStatusTool::new()),
        Box::new(GitDiffTool::new()),
        Box::new(GitLogTool::new()),
        Box::new(GitCommitTool::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_status_schema() {
        let tool = GitStatusTool::new();
        assert_eq!(tool.name(), "git_status");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
    }

    #[test]
    fn git_diff_schema() {
        let tool = GitDiffTool::new();
        assert_eq!(tool.name(), "git_diff");
        let schema = tool.input_schema();
        assert!(schema["properties"]["staged"].is_object());
        assert!(schema["properties"]["file"].is_object());
    }

    #[test]
    fn git_log_schema() {
        let tool = GitLogTool::new();
        assert_eq!(tool.name(), "git_log");
        let schema = tool.input_schema();
        assert!(schema["properties"]["count"].is_object());
        assert!(schema["properties"]["oneline"].is_object());
    }

    #[test]
    fn git_commit_schema() {
        let tool = GitCommitTool::new();
        assert_eq!(tool.name(), "git_commit");
        let schema = tool.input_schema();
        assert_eq!(schema["required"][0], "files");
        assert_eq!(schema["required"][1], "message");
    }

    #[tokio::test]
    async fn git_commit_rejects_empty_files() {
        let tool = GitCommitTool::new();
        let result = tool
            .execute(serde_json::json!({
                "files": [],
                "message": "test commit",
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must not be empty"));
    }
}
