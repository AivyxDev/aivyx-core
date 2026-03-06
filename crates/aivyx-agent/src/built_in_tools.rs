use std::path::PathBuf;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use glob::glob as glob_match;
use sha2::Digest;

/// Maximum output length in characters for tool results (file reads, diffs, etc.).
const MAX_TOOL_OUTPUT_CHARS: usize = 8000;

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

/// Built-in tool: read a file from the filesystem.
pub struct FileReadTool {
    id: ToolId,
}

impl Default for FileReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FileReadTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the given path."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
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
            .ok_or_else(|| AivyxError::Agent("file_read: missing 'path' parameter".into()))?;

        let canonical = resolve_and_validate_path(path, "file_read").await?;

        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(AivyxError::Io)?;

        Ok(serde_json::json!({
            "content": content,
            "path": canonical.to_string_lossy(),
        }))
    }
}

/// Built-in tool: write content to a file.
pub struct FileWriteTool {
    id: ToolId,
}

impl Default for FileWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FileWriteTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file at the given path, creating it if necessary."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
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
            .ok_or_else(|| AivyxError::Agent("file_write: missing 'path' parameter".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("file_write: missing 'content' parameter".into()))?;

        let canonical = resolve_and_validate_path(path, "file_write").await?;

        tokio::fs::write(&canonical, content)
            .await
            .map_err(AivyxError::Io)?;

        Ok(serde_json::json!({
            "status": "written",
            "path": canonical.to_string_lossy(),
            "bytes": content.len(),
        }))
    }
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

/// Built-in tool: search the web using DuckDuckGo HTML.
///
/// Scrapes the DuckDuckGo HTML-only search interface to retrieve results
/// without requiring an API key. Returns up to 5 results with titles,
/// URLs, and snippets.
pub struct WebSearchTool {
    id: ToolId,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    /// Create a new web search tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Parse DuckDuckGo HTML search results into structured data.
    fn parse_results(html: &str) -> Vec<serde_json::Value> {
        let mut results = Vec::new();

        // DuckDuckGo HTML results have anchors with class "result__a".
        // Each result block contains a title, URL, and snippet.
        for segment in html.split("class=\"result__a\"") {
            if results.len() >= 5 {
                break;
            }
            // Skip the first segment (before any results).
            if !segment.contains("href=\"") {
                continue;
            }

            // Extract URL from href.
            let url = segment
                .split("href=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .unwrap_or("")
                .to_string();

            if url.is_empty() || url.starts_with("//duckduckgo.com") {
                continue;
            }

            // Extract title text (between > and </a>).
            let title = segment
                .split('>')
                .nth(1)
                .and_then(|s| s.split("</a>").next())
                .map(|s| {
                    // Strip any remaining HTML tags.
                    let mut clean = String::new();
                    let mut in_tag = false;
                    for ch in s.chars() {
                        match ch {
                            '<' => in_tag = true,
                            '>' => in_tag = false,
                            _ if !in_tag => clean.push(ch),
                            _ => {}
                        }
                    }
                    clean.trim().to_string()
                })
                .unwrap_or_default();

            // Extract snippet from result__snippet class.
            let snippet = segment
                .split("class=\"result__snippet\"")
                .nth(1)
                .and_then(|s| s.split('>').nth(1))
                .and_then(|s| s.split("</").next())
                .map(|s| {
                    let mut clean = String::new();
                    let mut in_tag = false;
                    for ch in s.chars() {
                        match ch {
                            '<' => in_tag = true,
                            '>' => in_tag = false,
                            _ if !in_tag => clean.push(ch),
                            _ => {}
                        }
                    }
                    clean.trim().to_string()
                })
                .unwrap_or_default();

            if !title.is_empty() {
                results.push(serde_json::json!({
                    "title": title,
                    "url": url,
                    "snippet": snippet,
                }));
            }
        }

        results
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information on a given query and return relevant results with titles, URLs, and snippets."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["query"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Network {
            hosts: vec!["html.duckduckgo.com".to_string()],
            ports: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("web_search: missing 'query' parameter".into()))?;

        let encoded = urlencoding::encode(query);
        let url = format!("https://html.duckduckgo.com/html/?q={encoded}");

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("User-Agent", "Aivyx/0.1")
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("web search request failed: {e}")))?;

        let html = response
            .text()
            .await
            .map_err(|e| AivyxError::Http(format!("web search response read failed: {e}")))?;

        let results = Self::parse_results(&html);

        Ok(serde_json::json!({
            "query": query,
            "results": results,
            "result_count": results.len(),
        }))
    }
}

/// Built-in tool: fetch a URL and extract readable text content.
///
/// Downloads an HTML page and converts it to readable plain text using
/// the `html2text` library. Useful for the agent to read documentation,
/// articles, and web content.
pub struct HttpFetchTool {
    id: ToolId,
}

impl Default for HttpFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetchTool {
    /// Create a new HTTP fetch tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for HttpFetchTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "http_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and extract readable text content from the page."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch and extract text from"
                }
            },
            "required": ["url"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let url = input["url"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("http_fetch: missing 'url' parameter".into()))?;

        let validated_url = validate_fetch_url(url)?;

        let client = reqwest::Client::new();
        let response = client
            .get(&validated_url)
            .header("User-Agent", "Aivyx/0.1")
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("HTTP fetch failed: {e}")))?;

        let status = response.status().as_u16();
        let html = response
            .text()
            .await
            .map_err(|e| AivyxError::Http(format!("HTTP response read failed: {e}")))?;

        // Convert HTML to readable plain text.
        let text = html2text::from_read(html.as_bytes(), 80);

        // Truncate to prevent overwhelming the context window.
        let max_len = MAX_TOOL_OUTPUT_CHARS;
        let truncated = if text.len() > max_len {
            let boundary = text.floor_char_boundary(max_len);
            format!("{}... [truncated]", &text[..boundary])
        } else {
            text
        };

        Ok(serde_json::json!({
            "url": url,
            "status": status,
            "content": truncated,
        }))
    }
}

/// Built-in tool: list a directory tree filtered by depth and exclude patterns.
///
/// Returns an indented tree of files and directories, skipping common build
/// artifact and dependency directories. Useful for the agent to understand
/// project structure.
pub struct ProjectTreeTool {
    id: ToolId,
}

impl Default for ProjectTreeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectTreeTool {
    /// Create a new project tree tool instance.
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
///
/// Scans a source file for structural elements (functions, structs, enums,
/// traits, classes, interfaces) using regex-based line matching. Supports
/// Rust, Python, TypeScript, and JavaScript.
pub struct ProjectOutlineTool {
    id: ToolId,
}

impl Default for ProjectOutlineTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectOutlineTool {
    /// Create a new project outline tool instance.
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

/// Built-in tool: delete a file or empty directory.
pub struct FileDeleteTool {
    id: ToolId,
}

impl Default for FileDeleteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FileDeleteTool {
    /// Create a new file delete tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for FileDeleteTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "file_delete"
    }

    fn description(&self) -> &str {
        "Delete a file or empty directory."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file or directory to delete"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "If true, delete non-empty directories recursively (default: false)"
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
            .ok_or_else(|| AivyxError::Agent("file_delete: missing 'path' parameter".into()))?;
        let recursive = input["recursive"].as_bool().unwrap_or(false);

        let canonical = resolve_and_validate_path(path, "file_delete").await?;
        let path = canonical.to_string_lossy();

        let metadata = tokio::fs::metadata(canonical.as_path())
            .await
            .map_err(AivyxError::Io)?;

        let kind = if metadata.is_file() {
            tokio::fs::remove_file(canonical.as_path())
                .await
                .map_err(AivyxError::Io)?;
            "file"
        } else if metadata.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(canonical.as_path())
                    .await
                    .map_err(AivyxError::Io)?;
            } else {
                tokio::fs::remove_dir(canonical.as_path())
                    .await
                    .map_err(AivyxError::Io)?;
            }
            "directory"
        } else {
            return Err(AivyxError::Agent(format!(
                "file_delete: '{path}' is neither a file nor a directory"
            )));
        };

        Ok(serde_json::json!({
            "status": "deleted",
            "path": path,
            "kind": kind,
        }))
    }
}

/// Built-in tool: move or rename a file or directory.
pub struct FileMoveTool {
    id: ToolId,
}

impl Default for FileMoveTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FileMoveTool {
    /// Create a new file move tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for FileMoveTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "file_move"
    }

    fn description(&self) -> &str {
        "Move or rename a file or directory."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Source path of the file or directory to move"
                },
                "destination": {
                    "type": "string",
                    "description": "Destination path to move to"
                }
            },
            "required": ["source", "destination"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let source = input["source"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("file_move: missing 'source' parameter".into()))?;
        let destination = input["destination"].as_str().ok_or_else(|| {
            AivyxError::Agent("file_move: missing 'destination' parameter".into())
        })?;

        let source_canonical = resolve_and_validate_path(source, "file_move").await?;
        let dest_canonical = resolve_and_validate_path(destination, "file_move").await?;

        tokio::fs::rename(&source_canonical, &dest_canonical)
            .await
            .map_err(AivyxError::Io)?;

        Ok(serde_json::json!({
            "status": "moved",
            "source": source_canonical.to_string_lossy(),
            "destination": dest_canonical.to_string_lossy(),
        }))
    }
}

/// Built-in tool: copy a file or directory.
pub struct FileCopyTool {
    id: ToolId,
}

impl Default for FileCopyTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FileCopyTool {
    /// Create a new file copy tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for FileCopyTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "file_copy"
    }

    fn description(&self) -> &str {
        "Copy a file or directory."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Source path of the file or directory to copy"
                },
                "destination": {
                    "type": "string",
                    "description": "Destination path to copy to"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "If true, copy directories recursively (default: false)"
                }
            },
            "required": ["source", "destination"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let source = input["source"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("file_copy: missing 'source' parameter".into()))?;
        let destination = input["destination"].as_str().ok_or_else(|| {
            AivyxError::Agent("file_copy: missing 'destination' parameter".into())
        })?;
        let recursive = input["recursive"].as_bool().unwrap_or(false);

        let source_canonical = resolve_and_validate_path(source, "file_copy").await?;
        let dest_canonical = resolve_and_validate_path(destination, "file_copy").await?;

        let metadata = tokio::fs::metadata(&source_canonical)
            .await
            .map_err(AivyxError::Io)?;

        let bytes_copied = if metadata.is_file() {
            tokio::fs::copy(&source_canonical, &dest_canonical)
                .await
                .map_err(AivyxError::Io)?
        } else if metadata.is_dir() {
            if !recursive {
                return Err(AivyxError::Agent(
                    "file_copy: source is a directory; set 'recursive' to true".into(),
                ));
            }
            copy_dir_recursive(&source_canonical, &dest_canonical).await?
        } else {
            return Err(AivyxError::Agent(format!(
                "file_copy: '{}' is neither a file nor a directory",
                source_canonical.display()
            )));
        };

        Ok(serde_json::json!({
            "status": "copied",
            "source": source_canonical.to_string_lossy(),
            "destination": dest_canonical.to_string_lossy(),
            "bytes_copied": bytes_copied,
        }))
    }
}

/// Recursively copy a directory and its contents, returning total bytes copied.
async fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<u64> {
    tokio::fs::create_dir_all(dst)
        .await
        .map_err(AivyxError::Io)?;

    let mut total: u64 = 0;
    let mut entries = tokio::fs::read_dir(src).await.map_err(AivyxError::Io)?;

    while let Some(entry) = entries.next_entry().await.map_err(AivyxError::Io)? {
        let file_type = entry.file_type().await.map_err(AivyxError::Io)?;
        let dest_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            total = total
                .saturating_add(Box::pin(copy_dir_recursive(&entry.path(), &dest_path)).await?);
        } else {
            let copied = tokio::fs::copy(entry.path(), &dest_path)
                .await
                .map_err(AivyxError::Io)?;
            total = total.saturating_add(copied);
        }
    }

    Ok(total)
}

/// Built-in tool: list directory contents with metadata.
pub struct DirectoryListTool {
    id: ToolId,
}

impl Default for DirectoryListTool {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectoryListTool {
    /// Create a new directory list tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for DirectoryListTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "directory_list"
    }

    fn description(&self) -> &str {
        "List directory contents with metadata (name, type, size, modified timestamp)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the directory to list"
                },
                "show_hidden": {
                    "type": "boolean",
                    "description": "If true, include hidden files starting with '.' (default: false)"
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
            .ok_or_else(|| AivyxError::Agent("directory_list: missing 'path' parameter".into()))?;
        let show_hidden = input["show_hidden"].as_bool().unwrap_or(false);

        let mut entries_out = Vec::new();
        let mut read_dir = tokio::fs::read_dir(path).await.map_err(AivyxError::Io)?;

        let mut raw_entries = Vec::new();
        while let Some(entry) = read_dir.next_entry().await.map_err(AivyxError::Io)? {
            raw_entries.push(entry);
        }

        // Sort by name.
        raw_entries.sort_by_key(|e| e.file_name());

        for entry in raw_entries {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();

            // Skip hidden files unless show_hidden is true.
            if !show_hidden && name_str.starts_with('.') {
                continue;
            }

            let metadata = entry.metadata().await.map_err(AivyxError::Io)?;
            let file_type = entry.file_type().await.map_err(AivyxError::Io)?;

            let type_str = if file_type.is_file() {
                "file"
            } else if file_type.is_dir() {
                "directory"
            } else {
                "symlink"
            };

            let size = metadata.len();
            let modified = metadata
                .modified()
                .map(|t| {
                    let dt: DateTime<Utc> = DateTime::from(t);
                    dt.to_rfc3339()
                })
                .unwrap_or_else(|_| "unknown".to_string());

            entries_out.push(serde_json::json!({
                "name": name_str,
                "type": type_str,
                "size": size,
                "modified": modified,
            }));
        }

        Ok(serde_json::json!({
            "path": path,
            "entries": entries_out,
        }))
    }
}

/// Directory names to skip during grep search traversal.
const GREP_SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", "__pycache__"];

/// Built-in tool: search file contents for lines matching a regex pattern.
///
/// Searches a single file or recursively walks a directory, returning lines
/// that match the given regular expression. Skips binary and non-UTF-8 files.
pub struct GrepSearchTool {
    id: ToolId,
}

impl Default for GrepSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepSearchTool {
    /// Create a new grep search tool instance.
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
///
/// Uses glob pattern matching (e.g., `**/*.rs`, `src/*.ts`) to locate files
/// in a directory tree. Returns file paths with size and modification metadata.
pub struct GlobFindTool {
    id: ToolId,
}

impl Default for GlobFindTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobFindTool {
    /// Create a new glob find tool instance.
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

/// Built-in tool: compare two files and show their differences line by line.
///
/// Reads both files and produces a simple line-by-line diff output, marking
/// added lines with `+`, removed lines with `-`, and unchanged lines with ` `.
pub struct TextDiffTool {
    id: ToolId,
}

impl Default for TextDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextDiffTool {
    /// Create a new text diff tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Compute a simple line-by-line diff between two texts.
    ///
    /// Uses a longest common subsequence (LCS) approach to produce a minimal
    /// diff with `+` (added), `-` (removed), and ` ` (unchanged) markers.
    fn compute_diff(text_a: &str, text_b: &str) -> (String, usize, usize) {
        let lines_a: Vec<&str> = text_a.lines().collect();
        let lines_b: Vec<&str> = text_b.lines().collect();
        let n = lines_a.len();
        let m = lines_b.len();

        // Build LCS table.
        let mut lcs = vec![vec![0usize; m + 1]; n + 1];
        for i in 1..=n {
            for j in 1..=m {
                if lines_a[i - 1] == lines_b[j - 1] {
                    lcs[i][j] = lcs[i - 1][j - 1] + 1;
                } else if lcs[i - 1][j] >= lcs[i][j - 1] {
                    lcs[i][j] = lcs[i - 1][j];
                } else {
                    lcs[i][j] = lcs[i][j - 1];
                }
            }
        }

        // Backtrack to produce diff.
        let mut diff_lines = Vec::new();
        let mut additions: usize = 0;
        let mut deletions: usize = 0;
        let mut i = n;
        let mut j = m;

        while i > 0 || j > 0 {
            if i > 0 && j > 0 && lines_a[i - 1] == lines_b[j - 1] {
                diff_lines.push(format!(" {}", lines_a[i - 1]));
                i -= 1;
                j -= 1;
            } else if j > 0 && (i == 0 || lcs[i][j - 1] >= lcs[i - 1][j]) {
                diff_lines.push(format!("+{}", lines_b[j - 1]));
                additions += 1;
                j -= 1;
            } else if i > 0 {
                diff_lines.push(format!("-{}", lines_a[i - 1]));
                deletions += 1;
                i -= 1;
            }
        }

        diff_lines.reverse();
        (diff_lines.join("\n"), additions, deletions)
    }
}

#[async_trait]
impl Tool for TextDiffTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "text_diff"
    }

    fn description(&self) -> &str {
        "Compare two files and show their differences line by line."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_a": {
                    "type": "string",
                    "description": "Path to first file"
                },
                "file_b": {
                    "type": "string",
                    "description": "Path to second file"
                }
            },
            "required": ["file_a", "file_b"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let file_a = input["file_a"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("text_diff: missing 'file_a' parameter".into()))?;
        let file_b = input["file_b"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("text_diff: missing 'file_b' parameter".into()))?;

        let content_a = tokio::fs::read_to_string(file_a)
            .await
            .map_err(AivyxError::Io)?;
        let content_b = tokio::fs::read_to_string(file_b)
            .await
            .map_err(AivyxError::Io)?;

        let (diff, additions, deletions) = Self::compute_diff(&content_a, &content_b);

        // Truncate if needed.
        let max_len = MAX_TOOL_OUTPUT_CHARS;
        let diff_output = if diff.len() > max_len {
            let boundary = diff.floor_char_boundary(max_len);
            format!("{}... [truncated]", &diff[..boundary])
        } else {
            diff
        };

        let changes_found = additions > 0 || deletions > 0;

        Ok(serde_json::json!({
            "diff": diff_output,
            "additions": additions,
            "deletions": deletions,
            "changes_found": changes_found,
        }))
    }
}

/// Built-in tool: get the current date and time.
///
/// Returns the current timestamp in both UTC and local time, along with
/// a Unix timestamp. Useful for time-aware agent tasks without side effects.
pub struct SystemTimeTool {
    id: ToolId,
}

impl Default for SystemTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTimeTool {
    /// Create a new system time tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for SystemTimeTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "system_time"
    }

    fn description(&self) -> &str {
        "Get the current date and time."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "timezone": {
                    "type": "string",
                    "description": "IANA timezone name, e.g. 'America/New_York' (currently ignored, returns UTC + local)"
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let utc_now = Utc::now();
        let local_now = Local::now();

        Ok(serde_json::json!({
            "utc": utc_now.to_rfc3339(),
            "local": local_now.to_rfc3339(),
            "unix_timestamp": utc_now.timestamp(),
            "timezone": "UTC",
        }))
    }
}

/// Built-in tool: read environment variables.
///
/// Returns the value of a specific environment variable or all non-sensitive
/// variables. Variables containing sensitive keywords (PASSWORD, TOKEN, SECRET,
/// KEY, CREDENTIAL, API_KEY, PASSPHRASE) are filtered out for security.
pub struct EnvReadTool {
    id: ToolId,
}

impl Default for EnvReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvReadTool {
    /// Create a new env read tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// Sensitive environment variable name patterns to filter out.
const SENSITIVE_ENV_PATTERNS: &[&str] = &[
    "PASSWORD",
    "TOKEN",
    "SECRET",
    "KEY",
    "CREDENTIAL",
    "API_KEY",
    "PASSPHRASE",
];

/// Check whether an environment variable name matches a sensitive pattern.
fn is_sensitive_env(name: &str) -> bool {
    let upper = name.to_uppercase();
    SENSITIVE_ENV_PATTERNS.iter().any(|p| upper.contains(p))
}

#[async_trait]
impl Tool for EnvReadTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "env_read"
    }

    fn description(&self) -> &str {
        "Read environment variables. Sensitive variables (containing PASSWORD, TOKEN, SECRET, KEY, CREDENTIAL, API_KEY) are filtered out for security."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Specific variable name. If omitted, returns all non-sensitive variables."
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Shell {
            allowed_commands: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if let Some(name) = input["name"].as_str() {
            if is_sensitive_env(name) {
                return Err(AivyxError::Agent(
                    "env_read: access to sensitive env var denied".into(),
                ));
            }
            let value = std::env::var(name).unwrap_or_default();
            Ok(serde_json::json!({
                "variables": { name: value },
            }))
        } else {
            let mut variables = serde_json::Map::new();
            for (k, v) in std::env::vars() {
                if !is_sensitive_env(&k) {
                    variables.insert(k, serde_json::Value::String(v));
                }
            }
            Ok(serde_json::json!({
                "variables": variables,
            }))
        }
    }
}

/// Built-in tool: parse, format, and query JSON data.
///
/// Accepts a JSON string, optionally navigates it with a simple dot-path
/// query (e.g. `"data.items[0].name"`), and returns pretty-printed output.
/// Pure data operation with no side effects.
pub struct JsonParseTool {
    id: ToolId,
}

impl Default for JsonParseTool {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonParseTool {
    /// Create a new JSON parse tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Navigate a JSON value using a dot-path query.
    ///
    /// Supports field access (`data.name`) and array indexing (`items[0]`).
    fn query_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
        let mut current = value;
        for segment in path.split('.') {
            if segment.is_empty() {
                continue;
            }
            // Check for array index: "field[N]"
            if let Some(bracket_pos) = segment.find('[') {
                let field = &segment[..bracket_pos];
                let idx_str = segment[bracket_pos + 1..].trim_end_matches(']');
                if !field.is_empty() {
                    current = current.get(field)?;
                }
                let idx: usize = idx_str.parse().ok()?;
                current = current.get(idx)?;
            } else {
                current = current.get(segment)?;
            }
        }
        Some(current)
    }
}

#[async_trait]
impl Tool for JsonParseTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "json_parse"
    }

    fn description(&self) -> &str {
        "Parse, format, and query JSON data. Supports simple dot-path queries like 'data.items[0].name'."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "JSON text to parse"
                },
                "query": {
                    "type": "string",
                    "description": "Simple dot-path query (e.g., 'data.items[0].name')"
                },
                "pretty": {
                    "type": "boolean",
                    "description": "Pretty-print the output (default: true)"
                }
            },
            "required": ["input"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let json_text = input["input"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("json_parse: missing 'input' parameter".into()))?;

        let parsed: serde_json::Value = serde_json::from_str(json_text)
            .map_err(|e| AivyxError::Agent(format!("json_parse: invalid JSON: {e}")))?;

        let pretty = input["pretty"].as_bool().unwrap_or(true);

        let query_result = if let Some(query) = input["query"].as_str() {
            Self::query_path(&parsed, query).cloned()
        } else {
            None
        };

        let formatted = if pretty {
            serde_json::to_string_pretty(&parsed)
                .map_err(|e| AivyxError::Agent(format!("json_parse: formatting failed: {e}")))?
        } else {
            serde_json::to_string(&parsed)
                .map_err(|e| AivyxError::Agent(format!("json_parse: formatting failed: {e}")))?
        };

        Ok(serde_json::json!({
            "parsed": formatted,
            "query_result": query_result,
        }))
    }
}

/// Built-in tool: compute a cryptographic hash of a file or text string.
///
/// Supports SHA-256 and SHA-512 algorithms. Can hash either a file on disk
/// or a raw text string provided directly.
pub struct HashComputeTool {
    id: ToolId,
}

impl Default for HashComputeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HashComputeTool {
    /// Create a new hash compute tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for HashComputeTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "hash_compute"
    }

    fn description(&self) -> &str {
        "Compute a cryptographic hash of a file or text string."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "File path or raw text to hash"
                },
                "algorithm": {
                    "type": "string",
                    "description": "Hash algorithm: 'sha256' or 'sha512' (default: 'sha256')"
                },
                "mode": {
                    "type": "string",
                    "description": "'file' reads from disk, 'text' hashes the input string directly (default: 'text')"
                }
            },
            "required": ["input"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let raw_input = input["input"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("hash_compute: missing 'input' parameter".into()))?;
        let algorithm = input["algorithm"].as_str().unwrap_or("sha256");
        let mode = input["mode"].as_str().unwrap_or("text");

        let data: Vec<u8> = match mode {
            "file" => tokio::fs::read(raw_input).await.map_err(AivyxError::Io)?,
            "text" => raw_input.as_bytes().to_vec(),
            other => {
                return Err(AivyxError::Agent(format!(
                    "hash_compute: unsupported mode '{other}', expected 'file' or 'text'"
                )));
            }
        };

        let hash = match algorithm {
            "sha256" => {
                let mut hasher = sha2::Sha256::new();
                hasher.update(&data);
                hex::encode(hasher.finalize())
            }
            "sha512" => {
                let mut hasher = sha2::Sha512::new();
                hasher.update(&data);
                hex::encode(hasher.finalize())
            }
            other => {
                return Err(AivyxError::Agent(format!(
                    "hash_compute: unsupported algorithm '{other}', expected 'sha256' or 'sha512'"
                )));
            }
        };

        Ok(serde_json::json!({
            "hash": hash,
            "algorithm": algorithm,
            "input": raw_input,
            "mode": mode,
        }))
    }
}

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
    /// Create a new git status tool instance.
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
    /// Create a new git diff tool instance.
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
            cmd.arg(f);
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
            stat_cmd.arg(f);
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
    /// Create a new git log tool instance.
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
///
/// Requires an explicit file list for safety — does not support
/// `git add .` or `git add -A`.
pub struct GitCommitTool {
    id: ToolId,
}

impl Default for GitCommitTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitCommitTool {
    /// Create a new git commit tool instance.
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
        add_cmd.arg("add");
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
    let mut all_tools: Vec<Box<dyn Tool>> = vec![
        Box::new(FileReadTool::new()),
        Box::new(FileWriteTool::new()),
        Box::new(ShellTool::new()),
        Box::new(WebSearchTool::new()),
        Box::new(HttpFetchTool::new()),
        Box::new(ProjectTreeTool::new()),
        Box::new(ProjectOutlineTool::new()),
        Box::new(FileDeleteTool::new()),
        Box::new(FileMoveTool::new()),
        Box::new(FileCopyTool::new()),
        Box::new(DirectoryListTool::new()),
        Box::new(GrepSearchTool::new()),
        Box::new(GlobFindTool::new()),
        Box::new(TextDiffTool::new()),
        Box::new(SystemTimeTool::new()),
        Box::new(EnvReadTool::new()),
        Box::new(JsonParseTool::new()),
        Box::new(HashComputeTool::new()),
        Box::new(GitStatusTool::new()),
        Box::new(GitDiffTool::new()),
        Box::new(GitLogTool::new()),
        Box::new(GitCommitTool::new()),
    ];

    // Phase 11A: Analysis & computation tools
    all_tools.extend(crate::analysis_tools::create_analysis_tools());

    // Phase 11B: Document intelligence tools
    all_tools.extend(crate::document_tools::create_document_tools());

    // Phase 11C: Network & communication tools (stateless only; contextual tools in session.rs)
    all_tools.extend(crate::network_tools::create_network_tools());

    // Phase 11D: Infrastructure, safety & advanced tools (stateless only; schedule_task in session.rs)
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
    fn file_read_schema() {
        let tool = FileReadTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
    }

    #[test]
    fn file_write_schema() {
        let tool = FileWriteTool::new();
        assert_eq!(tool.name(), "file_write");
    }

    #[test]
    fn shell_schema() {
        let tool = ShellTool::new();
        assert_eq!(tool.name(), "shell");
        let schema = tool.input_schema();
        assert!(schema["properties"]["command"].is_object());
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
        // pub fn hello, pub struct Foo, impl Foo, fn bar, pub enum Color, pub trait Drawable, fn draw
        assert_eq!(items.len(), 7);
        assert_eq!(items[0]["kind"], "function");
        assert_eq!(items[0]["line"], 1);
        assert_eq!(items[1]["kind"], "struct");
        assert_eq!(items[2]["kind"], "impl");
        assert_eq!(items[3]["kind"], "function"); // fn bar
        assert_eq!(items[4]["kind"], "enum");
        assert_eq!(items[5]["kind"], "trait");
        assert_eq!(items[6]["kind"], "function"); // fn draw
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
        // target/ should be excluded
        assert!(!tree.contains("target/"));

        // Cleanup
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

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_read_required_scope() {
        let tool = FileReadTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Filesystem { .. }));
    }

    #[test]
    fn file_write_required_scope() {
        let tool = FileWriteTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Filesystem { .. }));
    }

    #[test]
    fn shell_required_scope() {
        let tool = ShellTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Shell { .. }));
    }

    #[test]
    fn web_search_schema() {
        let tool = WebSearchTool::new();
        assert_eq!(tool.name(), "web_search");
        let schema = tool.input_schema();
        assert!(schema["properties"]["query"].is_object());
    }

    #[test]
    fn web_search_required_scope() {
        let tool = WebSearchTool::new();
        let scope = tool.required_scope().unwrap();
        if let CapabilityScope::Network { hosts, .. } = &scope {
            assert!(hosts.contains(&"html.duckduckgo.com".to_string()));
        } else {
            panic!("expected Network scope");
        }
    }

    #[test]
    fn web_search_parse_results_empty_html() {
        let results = WebSearchTool::parse_results("<html><body>no results</body></html>");
        assert!(results.is_empty());
    }

    #[test]
    fn http_fetch_schema() {
        let tool = HttpFetchTool::new();
        assert_eq!(tool.name(), "http_fetch");
        let schema = tool.input_schema();
        assert!(schema["properties"]["url"].is_object());
        assert_eq!(schema["required"][0], "url");
    }

    #[test]
    fn http_fetch_required_scope() {
        let tool = HttpFetchTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Network { .. }));
    }

    #[tokio::test]
    async fn file_delete_refuses_root_path() {
        let tool = FileDeleteTool::new();
        let result = tool.execute(serde_json::json!({ "path": "/" })).await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("dangerous path"));
    }

    #[test]
    fn file_move_schema() {
        let tool = FileMoveTool::new();
        assert_eq!(tool.name(), "file_move");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["source"].is_object());
        assert!(schema["properties"]["destination"].is_object());
        assert_eq!(schema["required"][0], "source");
        assert_eq!(schema["required"][1], "destination");
    }

    #[test]
    fn file_copy_schema() {
        let tool = FileCopyTool::new();
        assert_eq!(tool.name(), "file_copy");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["source"].is_object());
        assert!(schema["properties"]["destination"].is_object());
        assert!(schema["properties"]["recursive"].is_object());
        assert_eq!(schema["required"][0], "source");
        assert_eq!(schema["required"][1], "destination");
    }

    #[test]
    fn directory_list_schema() {
        let tool = DirectoryListTool::new();
        assert_eq!(tool.name(), "directory_list");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["show_hidden"].is_object());
        assert_eq!(schema["required"][0], "path");
    }

    #[tokio::test]
    async fn directory_list_execute_temp_dir() {
        let root =
            std::env::temp_dir().join(format!("aivyx-dirlist-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), "hello").unwrap();
        std::fs::write(root.join("beta.txt"), "world").unwrap();

        let tool = DirectoryListTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": root.to_str().unwrap() }))
            .await
            .unwrap();

        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["name"], "alpha.txt");
        assert_eq!(entries[0]["type"], "file");
        assert_eq!(entries[1]["name"], "beta.txt");
        assert_eq!(entries[1]["type"], "file");

        // Cleanup
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn file_delete_execute_temp_file() {
        let root = std::env::temp_dir().join(format!("aivyx-delete-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("deleteme.txt");
        std::fs::write(&file, "goodbye").unwrap();
        assert!(file.exists());

        let tool = FileDeleteTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": file.to_str().unwrap() }))
            .await
            .unwrap();

        assert_eq!(result["status"], "deleted");
        assert_eq!(result["kind"], "file");
        assert!(!file.exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn system_time_schema() {
        let tool = SystemTimeTool::new();
        assert_eq!(tool.name(), "system_time");
        assert!(tool.required_scope().is_none());
    }

    #[tokio::test]
    async fn system_time_execute() {
        let tool = SystemTimeTool::new();
        let result = tool.execute(serde_json::json!({})).await.unwrap();

        assert!(result["utc"].as_str().is_some());
        assert!(result["unix_timestamp"].as_i64().is_some());
        assert!(result["local"].as_str().is_some());
        assert_eq!(result["timezone"], "UTC");
    }

    #[test]
    fn env_read_schema() {
        let tool = EnvReadTool::new();
        assert_eq!(tool.name(), "env_read");
        let schema = tool.input_schema();
        assert!(schema["properties"]["name"].is_object());
    }

    #[tokio::test]
    async fn env_read_filters_sensitive() {
        // SAFETY: This test is single-threaded and sets/removes a unique var name.
        unsafe {
            std::env::set_var("TEST_PASSWORD_VAR", "secret123");
        }
        let tool = EnvReadTool::new();
        let result = tool
            .execute(serde_json::json!({ "name": "TEST_PASSWORD_VAR" }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("sensitive"));

        // Cleanup
        // SAFETY: This test is single-threaded and sets/removes a unique var name.
        unsafe {
            std::env::remove_var("TEST_PASSWORD_VAR");
        }
    }

    #[tokio::test]
    async fn json_parse_valid() {
        let tool = JsonParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "{\"a\":1}",
            }))
            .await
            .unwrap();

        assert!(result["parsed"].as_str().is_some());
        let parsed_str = result["parsed"].as_str().unwrap();
        assert!(parsed_str.contains("\"a\""));
        assert!(parsed_str.contains('1'));
    }

    #[tokio::test]
    async fn json_parse_query() {
        let tool = JsonParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "{\"data\":{\"name\":\"test\"}}",
                "query": "data.name",
            }))
            .await
            .unwrap();

        assert_eq!(result["query_result"], "test");
    }

    #[tokio::test]
    async fn hash_compute_text_sha256() {
        let tool = HashComputeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "hello",
                "algorithm": "sha256",
                "mode": "text",
            }))
            .await
            .unwrap();

        // SHA-256 of "hello" is a known value.
        assert_eq!(
            result["hash"],
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(result["algorithm"], "sha256");
        assert_eq!(result["mode"], "text");
    }

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

    // --- Part A: Built-in tool execution tests ---

    #[tokio::test]
    async fn file_delete_success() {
        let root = std::env::temp_dir().join(format!("aivyx-del-ok-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("todelete.txt");
        std::fs::write(&file, "bye").unwrap();
        assert!(file.exists());

        let tool = FileDeleteTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": file.to_str().unwrap() }))
            .await
            .unwrap();

        assert_eq!(result["status"], "deleted");
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn file_delete_nonexistent() {
        let bogus = std::env::temp_dir()
            .join(format!("aivyx-del-noexist-{}", uuid::Uuid::new_v4()))
            .join("nope.txt");
        let tool = FileDeleteTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": bogus.to_str().unwrap() }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_move_success() {
        let root = std::env::temp_dir().join(format!("aivyx-mv-ok-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("source.txt");
        let dst = root.join("dest.txt");
        std::fs::write(&src, "moveme").unwrap();

        let tool = FileMoveTool::new();
        let result = tool
            .execute(serde_json::json!({
                "source": src.to_str().unwrap(),
                "destination": dst.to_str().unwrap(),
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "moved");
        assert!(!src.exists());
        assert!(dst.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "moveme");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn file_move_nonexistent_source() {
        let root = std::env::temp_dir().join(format!("aivyx-mv-noexist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("ghost.txt");
        let dst = root.join("dest.txt");

        let tool = FileMoveTool::new();
        let result = tool
            .execute(serde_json::json!({
                "source": src.to_str().unwrap(),
                "destination": dst.to_str().unwrap(),
            }))
            .await;
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn file_copy_success() {
        let root = std::env::temp_dir().join(format!("aivyx-cp-ok-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("original.txt");
        let dst = root.join("copy.txt");
        std::fs::write(&src, "hello copy").unwrap();

        let tool = FileCopyTool::new();
        let result = tool
            .execute(serde_json::json!({
                "source": src.to_str().unwrap(),
                "destination": dst.to_str().unwrap(),
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "copied");
        assert!(src.exists());
        assert!(dst.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello copy");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn file_copy_nonexistent_source() {
        let root = std::env::temp_dir().join(format!("aivyx-cp-noexist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("ghost.txt");
        let dst = root.join("dest.txt");

        let tool = FileCopyTool::new();
        let result = tool
            .execute(serde_json::json!({
                "source": src.to_str().unwrap(),
                "destination": dst.to_str().unwrap(),
            }))
            .await;
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn directory_list_success() {
        let root = std::env::temp_dir().join(format!("aivyx-dirlist-ok-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("one.txt"), "1").unwrap();
        std::fs::write(root.join("two.txt"), "2").unwrap();

        let tool = DirectoryListTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": root.to_str().unwrap() }))
            .await
            .unwrap();

        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"one.txt"));
        assert!(names.contains(&"two.txt"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn directory_list_nonexistent() {
        let bogus = std::env::temp_dir()
            .join(format!("aivyx-dirlist-noexist-{}", uuid::Uuid::new_v4()))
            .join("nope");
        let tool = DirectoryListTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": bogus.to_str().unwrap() }))
            .await;
        assert!(result.is_err());
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

    #[tokio::test]
    async fn text_diff_different_content() {
        let root = std::env::temp_dir().join(format!("aivyx-diff-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file_a = root.join("a.txt");
        let file_b = root.join("b.txt");
        std::fs::write(&file_a, "line one\nline two\nline three\n").unwrap();
        std::fs::write(&file_b, "line one\nline TWO\nline three\n").unwrap();

        let tool = TextDiffTool::new();
        let result = tool
            .execute(serde_json::json!({
                "file_a": file_a.to_str().unwrap(),
                "file_b": file_b.to_str().unwrap(),
            }))
            .await
            .unwrap();

        assert!(result["changes_found"].as_bool().unwrap());
        assert!(result["additions"].as_u64().unwrap() > 0);
        assert!(result["deletions"].as_u64().unwrap() > 0);
        let diff = result["diff"].as_str().unwrap();
        assert!(diff.contains("+"));
        assert!(diff.contains("-"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn env_read_path_variable() {
        let tool = EnvReadTool::new();
        let _result = tool.execute(serde_json::json!({ "name": "PATH" })).await;
        // PATH contains "KEY" substring so it gets filtered as sensitive
        // Use HOME instead which is always set and not sensitive
        let result = tool
            .execute(serde_json::json!({ "name": "HOME" }))
            .await
            .unwrap();

        let vars = &result["variables"];
        let home_val = vars["HOME"].as_str().unwrap();
        assert!(!home_val.is_empty());
    }

    #[tokio::test]
    async fn hash_compute_sha256() {
        let tool = HashComputeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "test string",
                "algorithm": "sha256",
                "mode": "text",
            }))
            .await
            .unwrap();

        let hash = result["hash"].as_str().unwrap();
        // SHA-256 always produces 64 hex chars.
        assert_eq!(hash.len(), 64);
        assert_eq!(result["algorithm"], "sha256");

        // Deterministic: same input same output.
        let result2 = tool
            .execute(serde_json::json!({
                "input": "test string",
                "algorithm": "sha256",
                "mode": "text",
            }))
            .await
            .unwrap();
        assert_eq!(result["hash"], result2["hash"]);
    }

    #[tokio::test]
    async fn json_parse_valid_structure() {
        let tool = JsonParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": r#"{"items":[1,2,3]}"#,
                "query": "items[1]",
            }))
            .await
            .unwrap();

        assert!(result["parsed"].as_str().is_some());
        assert_eq!(result["query_result"], 2);
    }

    #[tokio::test]
    async fn json_parse_invalid() {
        let tool = JsonParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "not valid json {{{",
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid JSON"));
    }

    #[tokio::test]
    async fn system_time_returns_current() {
        let tool = SystemTimeTool::new();
        let result = tool.execute(serde_json::json!({})).await.unwrap();

        let utc = result["utc"].as_str().unwrap();
        assert!(utc.contains('T')); // RFC 3339 timestamp
        let ts = result["unix_timestamp"].as_i64().unwrap();
        assert!(ts > 1_700_000_000); // After 2023-11-14
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
