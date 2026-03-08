//! Search and code analysis tools: grep, glob, project tree, and outline.

use std::path::PathBuf;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use glob::glob as glob_match;

use crate::built_in_tools::MAX_TOOL_OUTPUT_CHARS;

/// Built-in tool: list a directory tree filtered by depth and exclude patterns.
pub struct ProjectTreeTool {
    id: ToolId,
}

impl Default for ProjectTreeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectTreeTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// Default directory names to exclude from tree traversals.
const TREE_EXCLUDE_PATTERNS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "dist",
    "build",
    ".next",
    ".svelte-kit",
    ".mypy_cache",
    ".tox",
    ".eggs",
];

/// Maximum entries before truncation.
const MAX_TREE_ENTRIES: usize = 200;
/// Maximum output length in characters.
const MAX_TREE_CHARS: usize = 8000;

#[async_trait]
impl Tool for ProjectTreeTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "project_tree"
    }

    fn description(&self) -> &str {
        "List the directory structure of a project, filtering out common build and dependency directories. Returns an indented tree with file and directory names."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the directory to list"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth to traverse (default: 3)"
                }
            },
            "required": ["path"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("project_tree: missing 'path' parameter".into()))?;
        let max_depth = input["max_depth"].as_u64().unwrap_or(3) as usize;

        let root = std::path::Path::new(path);
        if !root.is_dir() {
            return Err(AivyxError::Agent(format!(
                "project_tree: '{path}' is not a directory"
            )));
        }

        let mut output = String::new();
        let mut entry_count = 0;
        walk_tree(root, max_depth, 0, &mut output, &mut entry_count)?;

        // Truncate if needed.
        if output.len() > MAX_TREE_CHARS {
            let boundary = output.floor_char_boundary(MAX_TREE_CHARS);
            output.truncate(boundary);
            output.push_str("\n... [truncated]");
        }

        Ok(serde_json::json!({
            "path": path,
            "tree": output,
            "entry_count": entry_count,
        }))
    }
}

/// Recursively walk a directory and build an indented tree string.
fn walk_tree(
    dir: &std::path::Path,
    max_depth: usize,
    depth: usize,
    output: &mut String,
    entry_count: &mut usize,
) -> Result<()> {
    if depth > max_depth || *entry_count >= MAX_TREE_ENTRIES {
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(AivyxError::Io)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        if *entry_count >= MAX_TREE_ENTRIES {
            break;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip excluded directories.
        if TREE_EXCLUDE_PATTERNS.iter().any(|p| *p == name_str) {
            continue;
        }
        // Skip hidden files (except project root level).
        if name_str.starts_with('.') && depth > 0 {
            continue;
        }

        let indent = "  ".repeat(depth);
        let file_type = entry.file_type().map_err(AivyxError::Io)?;
        if file_type.is_dir() {
            output.push_str(&format!("{indent}{name_str}/\n"));
            *entry_count += 1;
            walk_tree(&entry.path(), max_depth, depth + 1, output, entry_count)?;
        } else {
            output.push_str(&format!("{indent}{name_str}\n"));
            *entry_count += 1;
        }
    }

    Ok(())
}

/// Built-in tool: extract structural outlines from source files.
pub struct ProjectOutlineTool {
    id: ToolId,
}

impl Default for ProjectOutlineTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectOutlineTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Detect the language from a file extension.
    fn detect_language(path: &str) -> &'static str {
        if path.ends_with(".rs") {
            "rust"
        } else if path.ends_with(".py") {
            "python"
        } else if path.ends_with(".ts") || path.ends_with(".tsx") {
            "typescript"
        } else if path.ends_with(".js") || path.ends_with(".jsx") {
            "javascript"
        } else if path.ends_with(".go") {
            "go"
        } else {
            "unknown"
        }
    }

    /// Extract structural elements from a source file's lines.
    fn extract_outline(content: &str, language: &str) -> Vec<serde_json::Value> {
        let mut items = Vec::new();

        let patterns: &[(&str, &str)] = match language {
            "rust" => &[
                ("pub fn ", "function"),
                ("fn ", "function"),
                ("pub async fn ", "function"),
                ("async fn ", "function"),
                ("pub struct ", "struct"),
                ("struct ", "struct"),
                ("pub enum ", "enum"),
                ("enum ", "enum"),
                ("pub trait ", "trait"),
                ("trait ", "trait"),
                ("impl ", "impl"),
                ("pub mod ", "module"),
                ("mod ", "module"),
                ("pub type ", "type"),
                ("type ", "type"),
            ],
            "python" => &[
                ("async def ", "function"),
                ("def ", "function"),
                ("class ", "class"),
            ],
            "typescript" | "javascript" => &[
                ("export default function ", "function"),
                ("export function ", "function"),
                ("export async function ", "function"),
                ("export default class ", "class"),
                ("export class ", "class"),
                ("function ", "function"),
                ("async function ", "function"),
                ("class ", "class"),
                ("export interface ", "interface"),
                ("interface ", "interface"),
                ("export type ", "type"),
                ("type ", "type"),
            ],
            "go" => &[("func ", "function"), ("type ", "type")],
            _ => &[],
        };

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            // Skip comments.
            if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") {
                continue;
            }

            for &(pattern, kind) in patterns {
                if trimmed.starts_with(pattern) {
                    // Extract the signature up to the opening brace, colon, or end of line.
                    let sig = trimmed
                        .split('{')
                        .next()
                        .unwrap_or(trimmed)
                        .split(':')
                        .next()
                        .unwrap_or(trimmed)
                        .trim();
                    // For Rust, keep the full signature up to {
                    let sig = if language == "rust" {
                        trimmed.split('{').next().unwrap_or(trimmed).trim()
                    } else {
                        sig
                    };

                    items.push(serde_json::json!({
                        "line": line_num + 1,
                        "kind": kind,
                        "signature": sig,
                    }));
                    break; // Only match first pattern per line.
                }
            }
        }

        items
    }
}

#[async_trait]
impl Tool for ProjectOutlineTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "project_outline"
    }

    fn description(&self) -> &str {
        "Extract the structural outline (functions, structs, enums, traits, classes, interfaces) from a source code file with line numbers."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the source file to analyze"
                }
            },
            "required": ["path"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("project_outline: missing 'path' parameter".into()))?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(AivyxError::Io)?;

        let language = Self::detect_language(path);
        let items = Self::extract_outline(&content, language);

        // Format as readable text.
        let formatted: String = items
            .iter()
            .map(|item| format!("L{}: {} {}", item["line"], item["kind"], item["signature"]))
            .collect::<Vec<_>>()
            .join("\n");

        // Truncate if needed.
        let max_len = MAX_TOOL_OUTPUT_CHARS;
        let output = if formatted.len() > max_len {
            let boundary = formatted.floor_char_boundary(max_len);
            format!("{}... [truncated]", &formatted[..boundary])
        } else {
            formatted
        };

        Ok(serde_json::json!({
            "path": path,
            "language": language,
            "outline": output,
            "item_count": items.len(),
        }))
    }
}

/// Directory names to skip during grep search traversal.
const GREP_SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", "__pycache__"];

/// Built-in tool: search file contents for lines matching a regex pattern.
pub struct GrepSearchTool {
    id: ToolId,
}

impl Default for GrepSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepSearchTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// Recursively collect file paths from a directory, skipping excluded directories.
fn collect_files_recursive(
    dir: &std::path::Path,
    recursive: bool,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(AivyxError::Io)?;
    for entry in entries {
        let entry = entry.map_err(AivyxError::Io)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(AivyxError::Io)?;

        if file_type.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if GREP_SKIP_DIRS.iter().any(|p| *p == name_str.as_ref()) {
                continue;
            }
            if recursive {
                collect_files_recursive(&path, true, files)?;
            }
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

#[async_trait]
impl Tool for GrepSearchTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "grep_search"
    }

    fn description(&self) -> &str {
        "Search file contents for lines matching a regex pattern."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Recurse into subdirectories (default: true)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum matches to return (default: 50)"
                }
            },
            "required": ["pattern", "path"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let pattern = input["pattern"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("grep_search: missing 'pattern' parameter".into()))?;
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("grep_search: missing 'path' parameter".into()))?;
        let recursive = input["recursive"].as_bool().unwrap_or(true);
        let max_results = input["max_results"].as_u64().unwrap_or(50) as usize;

        if pattern.len() > 1000 {
            return Err(AivyxError::Agent(
                "grep_search: pattern too long (max 1000 characters)".into(),
            ));
        }
        let re = regex::RegexBuilder::new(pattern)
            .size_limit(1_000_000)
            .build()
            .map_err(|e| AivyxError::Agent(format!("grep_search: invalid regex pattern: {e}")))?;

        let search_path = std::path::Path::new(path);
        let mut files_to_search = Vec::new();

        if search_path.is_file() {
            files_to_search.push(search_path.to_path_buf());
        } else if search_path.is_dir() {
            collect_files_recursive(search_path, recursive, &mut files_to_search)?;
        } else {
            return Err(AivyxError::Agent(format!(
                "grep_search: '{path}' is neither a file nor a directory"
            )));
        }

        let mut matches = Vec::new();
        let mut total_matches: usize = 0;

        for file_path in &files_to_search {
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue, // Skip binary/non-UTF-8 files.
            };

            for (line_num, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    total_matches = total_matches.saturating_add(1);
                    if matches.len() < max_results {
                        matches.push(serde_json::json!({
                            "file": file_path.to_string_lossy(),
                            "line_number": line_num + 1,
                            "line_content": line,
                        }));
                    }
                }
            }
        }

        let truncated = total_matches > max_results;

        Ok(serde_json::json!({
            "pattern": pattern,
            "matches": matches,
            "total_matches": total_matches,
            "truncated": truncated,
        }))
    }
}

/// Built-in tool: find files matching a glob pattern.
pub struct GlobFindTool {
    id: ToolId,
}

impl Default for GlobFindTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobFindTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for GlobFindTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "glob_find"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern (e.g., '**/*.rs', 'src/*.ts')."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files"
                },
                "path": {
                    "type": "string",
                    "description": "Root directory for the search (default: '.')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum files to return (default: 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let pattern = input["pattern"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("glob_find: missing 'pattern' parameter".into()))?;
        let path = input["path"].as_str().unwrap_or(".");
        let max_results = input["max_results"].as_u64().unwrap_or(100) as usize;

        // Build the full glob pattern.
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{}", path, pattern)
        };

        let glob_iter = glob_match(&full_pattern)
            .map_err(|e| AivyxError::Agent(format!("glob_find: invalid glob pattern: {e}")))?;

        let mut files = Vec::new();
        let mut total_found: usize = 0;

        for entry in glob_iter {
            let entry_path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };

            total_found = total_found.saturating_add(1);

            if files.len() < max_results {
                let metadata = std::fs::metadata(&entry_path);
                let (size, modified) = match metadata {
                    Ok(m) => {
                        let size = m.len();
                        let modified = m
                            .modified()
                            .map(|t| {
                                let dt: DateTime<Utc> = DateTime::from(t);
                                dt.to_rfc3339()
                            })
                            .unwrap_or_else(|_| "unknown".to_string());
                        (size, modified)
                    }
                    Err(_) => (0, "unknown".to_string()),
                };

                files.push(serde_json::json!({
                    "path": entry_path.to_string_lossy(),
                    "size": size,
                    "modified": modified,
                }));
            }
        }

        let truncated = total_found > max_results;

        Ok(serde_json::json!({
            "pattern": full_pattern,
            "files": files,
            "total_found": total_found,
            "truncated": truncated,
        }))
    }
}

/// Create all search tools.
pub fn create_search_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ProjectTreeTool::new()),
        Box::new(ProjectOutlineTool::new()),
        Box::new(GrepSearchTool::new()),
        Box::new(GlobFindTool::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_tree_schema() {
        let tool = ProjectTreeTool::new();
        assert_eq!(tool.name(), "project_tree");
        let schema = tool.input_schema();
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["max_depth"].is_object());
        assert_eq!(schema["required"][0], "path");
    }

    #[test]
    fn project_tree_required_scope() {
        let tool = ProjectTreeTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Filesystem { .. }));
    }

    #[test]
    fn project_outline_schema() {
        let tool = ProjectOutlineTool::new();
        assert_eq!(tool.name(), "project_outline");
        let schema = tool.input_schema();
        assert!(schema["properties"]["path"].is_object());
        assert_eq!(schema["required"][0], "path");
    }

    #[test]
    fn project_outline_required_scope() {
        let tool = ProjectOutlineTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Filesystem { .. }));
    }

    #[test]
    fn detect_language_from_extension() {
        assert_eq!(ProjectOutlineTool::detect_language("foo.rs"), "rust");
        assert_eq!(ProjectOutlineTool::detect_language("bar.py"), "python");
        assert_eq!(ProjectOutlineTool::detect_language("baz.ts"), "typescript");
        assert_eq!(ProjectOutlineTool::detect_language("qux.tsx"), "typescript");
        assert_eq!(ProjectOutlineTool::detect_language("app.js"), "javascript");
        assert_eq!(ProjectOutlineTool::detect_language("app.jsx"), "javascript");
        assert_eq!(ProjectOutlineTool::detect_language("main.go"), "go");
        assert_eq!(ProjectOutlineTool::detect_language("readme.md"), "unknown");
    }

    #[test]
    fn extract_outline_rust() {
        let src = "\
pub fn hello(name: &str) -> String {
    format!(\"hello {name}\")
}

pub struct Foo {
    x: i32,
}

impl Foo {
    fn bar(&self) -> i32 {
        self.x
    }
}

pub enum Color {
    Red,
    Green,
}

pub trait Drawable {
    fn draw(&self);
}
";
        let items = ProjectOutlineTool::extract_outline(src, "rust");
        assert_eq!(items.len(), 7);
        assert_eq!(items[0]["kind"], "function");
        assert_eq!(items[0]["line"], 1);
        assert_eq!(items[1]["kind"], "struct");
        assert_eq!(items[2]["kind"], "impl");
        assert_eq!(items[3]["kind"], "function");
        assert_eq!(items[4]["kind"], "enum");
        assert_eq!(items[5]["kind"], "trait");
        assert_eq!(items[6]["kind"], "function");
    }

    #[test]
    fn extract_outline_python() {
        let src = "\
class MyClass:
    def __init__(self):
        pass

    async def fetch(self):
        pass

def standalone():
    pass
";
        let items = ProjectOutlineTool::extract_outline(src, "python");
        assert_eq!(items.len(), 4);
        assert_eq!(items[0]["kind"], "class");
        assert_eq!(items[1]["kind"], "function");
        assert_eq!(items[2]["kind"], "function");
        assert_eq!(items[3]["kind"], "function");
    }

    #[test]
    fn extract_outline_skips_comments() {
        let src = "\
// pub fn commented_out() {}
pub fn real_function() {
}
";
        let items = ProjectOutlineTool::extract_outline(src, "rust");
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0]["signature"].as_str().unwrap(),
            "pub fn real_function()"
        );
    }

    #[tokio::test]
    async fn project_tree_execute_temp_dir() {
        let root = std::env::temp_dir().join(format!("aivyx-tree-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/bin"), "").unwrap();

        let tool = ProjectTreeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "path": root.to_str().unwrap(),
                "max_depth": 3,
            }))
            .await
            .unwrap();

        let tree = result["tree"].as_str().unwrap();
        assert!(tree.contains("src/"));
        assert!(tree.contains("main.rs"));
        assert!(tree.contains("Cargo.toml"));
        assert!(!tree.contains("target/"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn project_outline_execute_rust_file() {
        let dir = std::env::temp_dir().join(format!("aivyx-outline-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(
            &file,
            "pub struct Agent {\n    name: String,\n}\n\nimpl Agent {\n    pub fn new() -> Self {\n        todo!()\n    }\n}\n",
        )
        .unwrap();

        let tool = ProjectOutlineTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": file.to_str().unwrap() }))
            .await
            .unwrap();

        assert_eq!(result["language"], "rust");
        assert!(result["item_count"].as_u64().unwrap() >= 3);
        let outline = result["outline"].as_str().unwrap();
        assert!(outline.contains("struct"));
        assert!(outline.contains("impl"));
        assert!(outline.contains("function"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn grep_search_basic() {
        let root = std::env::temp_dir().join(format!("aivyx-grep-ok-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("haystack.txt");
        std::fs::write(&file, "line one\nfind_me here\nline three\n").unwrap();

        let tool = GrepSearchTool::new();
        let result = tool
            .execute(serde_json::json!({
                "pattern": "find_me",
                "path": file.to_str().unwrap(),
            }))
            .await
            .unwrap();

        assert_eq!(result["total_matches"], 1);
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert!(
            matches[0]["line_content"]
                .as_str()
                .unwrap()
                .contains("find_me")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn grep_search_no_matches() {
        let root = std::env::temp_dir().join(format!("aivyx-grep-none-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("haystack.txt");
        std::fs::write(&file, "nothing interesting here\n").unwrap();

        let tool = GrepSearchTool::new();
        let result = tool
            .execute(serde_json::json!({
                "pattern": "zzz_not_present",
                "path": file.to_str().unwrap(),
            }))
            .await
            .unwrap();

        assert_eq!(result["total_matches"], 0);
        assert!(result["matches"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn glob_find_matches() {
        let root = std::env::temp_dir().join(format!("aivyx-glob-ok-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), "a").unwrap();
        std::fs::write(root.join("beta.txt"), "b").unwrap();
        std::fs::write(root.join("gamma.rs"), "c").unwrap();

        let tool = GlobFindTool::new();
        let result = tool
            .execute(serde_json::json!({
                "pattern": "*.txt",
                "path": root.to_str().unwrap(),
            }))
            .await
            .unwrap();

        assert_eq!(result["total_found"], 2);
        let files = result["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn glob_find_no_matches() {
        let root = std::env::temp_dir().join(format!("aivyx-glob-none-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("only.rs"), "fn main() {}").unwrap();

        let tool = GlobFindTool::new();
        let result = tool
            .execute(serde_json::json!({
                "pattern": "*.py",
                "path": root.to_str().unwrap(),
            }))
            .await
            .unwrap();

        assert_eq!(result["total_found"], 0);
        assert!(result["files"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
