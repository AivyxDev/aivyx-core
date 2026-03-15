//! Web and text comparison tools: web search, HTTP fetch, and text diff.

use std::path::PathBuf;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;

use crate::built_in_tools::{MAX_TOOL_OUTPUT_CHARS, validate_fetch_url};

/// Built-in tool: search the web using DuckDuckGo HTML.
pub struct WebSearchTool {
    id: ToolId,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Parse DuckDuckGo HTML search results into structured data.
    fn parse_results(html: &str) -> Vec<serde_json::Value> {
        let mut results = Vec::new();

        for segment in html.split("class=\"result__a\"") {
            if results.len() >= 5 {
                break;
            }
            if !segment.contains("href=\"") {
                continue;
            }

            let url = segment
                .split("href=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .unwrap_or("")
                .to_string();

            if url.is_empty() || url.starts_with("//duckduckgo.com") {
                continue;
            }

            let title = segment
                .split('>')
                .nth(1)
                .and_then(|s| s.split("</a>").next())
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
pub struct HttpFetchTool {
    id: ToolId,
}

impl Default for HttpFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetchTool {
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

        let text = html2text::from_read(html.as_bytes(), 80);

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

/// Built-in tool: compare two files and show their differences line by line.
pub struct TextDiffTool {
    id: ToolId,
}

impl Default for TextDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextDiffTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Compute a simple line-by-line diff between two texts.
    fn compute_diff(text_a: &str, text_b: &str) -> (String, usize, usize) {
        let lines_a: Vec<&str> = text_a.lines().collect();
        let lines_b: Vec<&str> = text_b.lines().collect();
        let n = lines_a.len();
        let m = lines_b.len();

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

        // Guard against O(n*m) memory exhaustion in the LCS algorithm.
        // 2 000 lines per file ≈ 4M table entries (32 MB) at the limit.
        const MAX_DIFF_LINES: usize = 2_000;
        let lines_a = content_a.lines().count();
        let lines_b = content_b.lines().count();
        if lines_a > MAX_DIFF_LINES || lines_b > MAX_DIFF_LINES {
            return Err(AivyxError::Validation(format!(
                "text_diff: files too large for line-by-line diff \
                 ({lines_a} + {lines_b} lines, max {MAX_DIFF_LINES} per file)"
            )));
        }

        let (diff, additions, deletions) = Self::compute_diff(&content_a, &content_b);

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

/// Create all web tools.
pub fn create_web_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(WebSearchTool::new()),
        Box::new(HttpFetchTool::new()),
        Box::new(TextDiffTool::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
