//! Phase 11C: Network, communication & integration tools.
//!
//! Contains 5 tools split into two categories:
//! - **Stateless** (2): [`HttpRequestTool`], [`WebScrapeTool`] — registered via
//!   [`create_network_tools()`] factory in `register_built_in_tools()`.
//! - **Contextual** (3): [`TranslateTool`], [`NotificationSendTool`], [`EmailSendTool`] —
//!   registered per-session in `session.rs` because they need access to `EncryptedStore`,
//!   LLM providers, or SMTP configuration.

use std::collections::HashMap;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;

use crate::built_in_tools::validate_fetch_url;

/// Maximum output length in characters for tool results.
const MAX_TOOL_OUTPUT_CHARS: usize = 8000;

// ─────────────────────────────────────────────────────────────────────────────
// 1. HttpRequestTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Full HTTP client supporting GET, POST, PUT, PATCH, DELETE, HEAD with custom
/// headers, body, auth, and configurable timeout.
pub struct HttpRequestTool {
    id: ToolId,
}

impl Default for HttpRequestTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpRequestTool {
    /// Create a new HTTP request tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "http_request"
    }

    fn description(&self) -> &str {
        "Make an HTTP request with full control over method, headers, body, and authentication. \
         Supports GET, POST, PUT, PATCH, DELETE, HEAD. Returns raw response body (not \
         HTML-to-text converted like http_fetch)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to send the request to"
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"],
                    "description": "HTTP method (default: GET)"
                },
                "headers": {
                    "type": "object",
                    "description": "Custom request headers as key-value pairs"
                },
                "body": {
                    "type": "string",
                    "description": "Request body content"
                },
                "body_type": {
                    "type": "string",
                    "enum": ["json", "form", "text"],
                    "description": "Body content type (default: json). Sets Content-Type header."
                },
                "auth_header": {
                    "type": "string",
                    "description": "Value for the Authorization header (e.g. 'Bearer <token>')"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Request timeout in seconds (1-120, default: 30)"
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
            .ok_or_else(|| AivyxError::Agent("http_request: missing 'url' parameter".into()))?;

        let validated_url = validate_fetch_url(url)?;

        let method_str = input["method"].as_str().unwrap_or("GET");
        let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(30).clamp(1, 120);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| AivyxError::Agent(format!("http_request: client build error: {e}")))?;

        let mut request = match method_str.to_uppercase().as_str() {
            "GET" => client.get(&validated_url),
            "POST" => client.post(&validated_url),
            "PUT" => client.put(&validated_url),
            "PATCH" => client.patch(&validated_url),
            "DELETE" => client.delete(&validated_url),
            "HEAD" => client.head(&validated_url),
            other => {
                return Err(AivyxError::Agent(format!(
                    "http_request: unsupported method '{other}'"
                )));
            }
        };

        request = request.header("User-Agent", "Aivyx/0.1");

        // Custom headers
        if let Some(headers) = input["headers"].as_object() {
            for (key, val) in headers {
                if let Some(v) = val.as_str() {
                    request = request.header(key.as_str(), v);
                }
            }
        }

        // Auth header
        if let Some(auth) = input["auth_header"].as_str() {
            request = request.header("Authorization", auth);
        }

        // Body
        if let Some(body) = input["body"].as_str() {
            let body_type = input["body_type"].as_str().unwrap_or("json");
            request = match body_type {
                "form" => request
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body.to_string()),
                "text" => request
                    .header("Content-Type", "text/plain")
                    .body(body.to_string()),
                _ => request
                    .header("Content-Type", "application/json")
                    .body(body.to_string()),
            };
        }

        let response = request
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("http_request failed: {e}")))?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        // Collect response headers
        let resp_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // HEAD requests have no body
        let body = if method_str.to_uppercase() == "HEAD" {
            String::new()
        } else {
            response
                .text()
                .await
                .map_err(|e| AivyxError::Http(format!("http_request: response read failed: {e}")))?
        };

        let truncated = body.len() > MAX_TOOL_OUTPUT_CHARS;
        let body_out = if truncated {
            let boundary = body.floor_char_boundary(MAX_TOOL_OUTPUT_CHARS);
            format!("{}... [truncated]", &body[..boundary])
        } else {
            body
        };

        Ok(serde_json::json!({
            "url": url,
            "method": method_str.to_uppercase(),
            "status": status,
            "headers": resp_headers,
            "body": body_out,
            "content_type": content_type,
            "truncated": truncated,
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. WebScrapeTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch a URL and extract structured data using CSS selectors. Supports
/// multiple extraction modes: article, tables, links, images, metadata, css.
pub struct WebScrapeTool {
    id: ToolId,
}

impl Default for WebScrapeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebScrapeTool {
    /// Create a new web scraping tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for WebScrapeTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "web_scrape"
    }

    fn description(&self) -> &str {
        "Fetch a URL and extract structured data. Supports multiple modes: 'article' (main \
         content text), 'tables' (HTML tables as JSON), 'links' (all hrefs), 'images' (all \
         img srcs), 'metadata' (title, meta tags, OG tags), 'css' (custom CSS selector)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch and scrape"
                },
                "extract": {
                    "type": "string",
                    "enum": ["article", "tables", "links", "images", "metadata", "css"],
                    "description": "Extraction mode (default: article)"
                },
                "css_selector": {
                    "type": "string",
                    "description": "CSS selector to use (required when extract='css')"
                },
                "max_items": {
                    "type": "integer",
                    "description": "Maximum items to return (default: 100)"
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
            .ok_or_else(|| AivyxError::Agent("web_scrape: missing 'url' parameter".into()))?;

        let validated_url = validate_fetch_url(url)?;
        let mode = input["extract"].as_str().unwrap_or("article");
        let max_items = input["max_items"].as_u64().unwrap_or(100) as usize;

        let client = reqwest::Client::new();
        let html = client
            .get(&validated_url)
            .header("User-Agent", "Aivyx/0.1")
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("web_scrape: fetch failed: {e}")))?
            .text()
            .await
            .map_err(|e| AivyxError::Http(format!("web_scrape: response read failed: {e}")))?;

        let document = scraper::Html::parse_document(&html);

        match mode {
            "article" => extract_article(&document, url),
            "tables" => extract_tables(&document, url, max_items),
            "links" => extract_links(&document, url, max_items),
            "images" => extract_images(&document, url, max_items),
            "metadata" => extract_metadata(&document, url),
            "css" => {
                let selector_str = input["css_selector"].as_str().ok_or_else(|| {
                    AivyxError::Agent(
                        "web_scrape: 'css_selector' is required when extract='css'".into(),
                    )
                })?;
                extract_css(&document, url, selector_str, max_items)
            }
            other => Err(AivyxError::Agent(format!(
                "web_scrape: unknown extract mode '{other}'"
            ))),
        }
    }
}

fn extract_article(doc: &scraper::Html, url: &str) -> Result<serde_json::Value> {
    // Try common article selectors, fall back to body
    let selectors = [
        "article",
        "main",
        "[role=main]",
        ".content",
        "#content",
        "body",
    ];
    let mut text = String::new();

    for sel_str in selectors {
        if let Ok(selector) = scraper::Selector::parse(sel_str) {
            for element in doc.select(&selector) {
                text = element.text().collect::<Vec<_>>().join(" ");
                if !text.trim().is_empty() {
                    break;
                }
            }
            if !text.trim().is_empty() {
                break;
            }
        }
    }

    // Clean up whitespace
    let cleaned: String = text.split_whitespace().collect::<Vec<_>>().join(" ");

    let truncated = cleaned.len() > MAX_TOOL_OUTPUT_CHARS;
    let content = if truncated {
        let boundary = cleaned.floor_char_boundary(MAX_TOOL_OUTPUT_CHARS);
        format!("{}... [truncated]", &cleaned[..boundary])
    } else {
        cleaned
    };

    Ok(serde_json::json!({
        "url": url,
        "mode": "article",
        "content": content,
        "truncated": truncated,
    }))
}

fn extract_tables(doc: &scraper::Html, url: &str, max_items: usize) -> Result<serde_json::Value> {
    let table_sel = scraper::Selector::parse("table").map_err(|e| {
        AivyxError::Agent(format!("web_scrape: failed to parse 'table' selector: {e}"))
    })?;
    let tr_sel = scraper::Selector::parse("tr").map_err(|e| {
        AivyxError::Agent(format!("web_scrape: failed to parse 'tr' selector: {e}"))
    })?;
    let th_sel = scraper::Selector::parse("th").map_err(|e| {
        AivyxError::Agent(format!("web_scrape: failed to parse 'th' selector: {e}"))
    })?;
    let td_sel = scraper::Selector::parse("td").map_err(|e| {
        AivyxError::Agent(format!("web_scrape: failed to parse 'td' selector: {e}"))
    })?;

    let mut tables = Vec::new();

    for table_el in doc.select(&table_sel).take(max_items) {
        let mut rows = Vec::new();
        let mut headers: Vec<String> = Vec::new();

        for (i, tr) in table_el.select(&tr_sel).enumerate() {
            if i == 0 {
                // Check if first row has <th> elements
                let ths: Vec<String> = tr
                    .select(&th_sel)
                    .map(|th| th.text().collect::<Vec<_>>().join(" ").trim().to_string())
                    .collect();
                if !ths.is_empty() {
                    headers = ths;
                    continue;
                }
            }

            let cells: Vec<String> = tr
                .select(&td_sel)
                .map(|td| td.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .collect();

            if !cells.is_empty() {
                if !headers.is_empty() {
                    let mut row_map = serde_json::Map::new();
                    for (j, cell) in cells.iter().enumerate() {
                        let key = headers
                            .get(j)
                            .cloned()
                            .unwrap_or_else(|| format!("col_{j}"));
                        row_map.insert(key, serde_json::Value::String(cell.clone()));
                    }
                    rows.push(serde_json::Value::Object(row_map));
                } else {
                    rows.push(serde_json::json!(cells));
                }
            }
        }

        tables.push(serde_json::json!({
            "headers": headers,
            "rows": rows,
        }));
    }

    Ok(serde_json::json!({
        "url": url,
        "mode": "tables",
        "tables": tables,
        "count": tables.len(),
    }))
}

fn extract_links(doc: &scraper::Html, url: &str, max_items: usize) -> Result<serde_json::Value> {
    let a_sel = scraper::Selector::parse("a[href]").map_err(|e| {
        AivyxError::Agent(format!(
            "web_scrape: failed to parse 'a[href]' selector: {e}"
        ))
    })?;

    let links: Vec<serde_json::Value> = doc
        .select(&a_sel)
        .take(max_items)
        .filter_map(|el| {
            let href = el.value().attr("href")?;
            let text = el.text().collect::<Vec<_>>().join(" ");
            Some(serde_json::json!({
                "href": href,
                "text": text.trim(),
            }))
        })
        .collect();

    Ok(serde_json::json!({
        "url": url,
        "mode": "links",
        "links": links,
        "count": links.len(),
    }))
}

fn extract_images(doc: &scraper::Html, url: &str, max_items: usize) -> Result<serde_json::Value> {
    let img_sel = scraper::Selector::parse("img[src]").map_err(|e| {
        AivyxError::Agent(format!(
            "web_scrape: failed to parse 'img[src]' selector: {e}"
        ))
    })?;

    let images: Vec<serde_json::Value> = doc
        .select(&img_sel)
        .take(max_items)
        .filter_map(|el| {
            let src = el.value().attr("src")?;
            let alt = el.value().attr("alt").unwrap_or("");
            Some(serde_json::json!({
                "src": src,
                "alt": alt,
            }))
        })
        .collect();

    Ok(serde_json::json!({
        "url": url,
        "mode": "images",
        "images": images,
        "count": images.len(),
    }))
}

fn extract_metadata(doc: &scraper::Html, url: &str) -> Result<serde_json::Value> {
    let title_sel = scraper::Selector::parse("title").map_err(|e| {
        AivyxError::Agent(format!("web_scrape: failed to parse 'title' selector: {e}"))
    })?;
    let meta_sel = scraper::Selector::parse("meta").map_err(|e| {
        AivyxError::Agent(format!("web_scrape: failed to parse 'meta' selector: {e}"))
    })?;

    let title = doc
        .select(&title_sel)
        .next()
        .map(|el| el.text().collect::<Vec<_>>().join(""));

    let mut meta_tags = Vec::new();
    let mut og_tags = serde_json::Map::new();

    for el in doc.select(&meta_sel) {
        let name = el
            .value()
            .attr("name")
            .or_else(|| el.value().attr("property"));
        let content = el.value().attr("content");

        if let (Some(n), Some(c)) = (name, content) {
            if n.starts_with("og:") {
                og_tags.insert(
                    n.strip_prefix("og:").unwrap_or(n).to_string(),
                    serde_json::Value::String(c.to_string()),
                );
            } else {
                meta_tags.push(serde_json::json!({
                    "name": n,
                    "content": c,
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "url": url,
        "mode": "metadata",
        "title": title,
        "meta_tags": meta_tags,
        "og_tags": og_tags,
    }))
}

fn extract_css(
    doc: &scraper::Html,
    url: &str,
    selector_str: &str,
    max_items: usize,
) -> Result<serde_json::Value> {
    let selector = scraper::Selector::parse(selector_str).map_err(|e| {
        AivyxError::Agent(format!(
            "web_scrape: invalid CSS selector '{selector_str}': {e}"
        ))
    })?;

    let elements: Vec<serde_json::Value> = doc
        .select(&selector)
        .take(max_items)
        .map(|el| {
            let text = el.text().collect::<Vec<_>>().join(" ");
            let html = el.html();
            serde_json::json!({
                "text": text.trim(),
                "html": if html.len() > 2000 {
                    format!("{}... [truncated]", &html[..html.floor_char_boundary(2000)])
                } else {
                    html
                },
            })
        })
        .collect();

    Ok(serde_json::json!({
        "url": url,
        "mode": "css",
        "selector": selector_str,
        "elements": elements,
        "count": elements.len(),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. TranslateTool (contextual — needs LLM provider)
// ─────────────────────────────────────────────────────────────────────────────

/// Translate text between languages using the configured LLM provider.
///
/// This is a contextual tool: it needs access to `AivyxDirs`, `ProviderConfig`,
/// and a derived `MasterKey` to open the encrypted store and create an LLM
/// provider at execution time.
pub struct TranslateTool {
    id: ToolId,
    dirs: aivyx_config::AivyxDirs,
    provider_config: aivyx_config::ProviderConfig,
    master_key: aivyx_crypto::MasterKey,
}

impl TranslateTool {
    /// Create a new translate tool instance.
    pub fn new(
        dirs: aivyx_config::AivyxDirs,
        provider_config: aivyx_config::ProviderConfig,
        master_key: aivyx_crypto::MasterKey,
    ) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
            provider_config,
            master_key,
        }
    }
}

#[async_trait]
impl Tool for TranslateTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "translate"
    }

    fn description(&self) -> &str {
        "Translate text between languages using the configured LLM. Auto-detects source \
         language if not specified."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to translate"
                },
                "source_language": {
                    "type": "string",
                    "description": "Source language (auto-detected if omitted)"
                },
                "target_language": {
                    "type": "string",
                    "description": "Target language (e.g. 'English', 'Spanish', 'French')"
                }
            },
            "required": ["text", "target_language"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("translate: missing 'text' parameter".into()))?;
        let target = input["target_language"].as_str().ok_or_else(|| {
            AivyxError::Agent("translate: missing 'target_language' parameter".into())
        })?;
        let source = input["source_language"].as_str();

        let store = aivyx_crypto::EncryptedStore::open(self.dirs.store_path())?;
        let provider = aivyx_llm::create_provider(&self.provider_config, &store, &self.master_key)?;

        let system_prompt = if let Some(src) = source {
            format!(
                "You are a precise translator. Translate the following text from {src} to {target}. \
                 Output ONLY the translated text, nothing else."
            )
        } else {
            format!(
                "You are a precise translator. Detect the source language and translate the \
                 following text to {target}. Output ONLY the translated text on the first line. \
                 On the second line, output 'Source: <detected_language>'."
            )
        };

        let messages = vec![aivyx_llm::ChatMessage::user(text)];

        let request = aivyx_llm::ChatRequest {
            system_prompt: Some(system_prompt),
            messages,
            max_tokens: 4096,
            model: None,
            tools: vec![],
        };

        let response = provider.chat(&request).await?;

        let output = response.message.content.text().trim().to_string();

        // If source was auto-detected, try to parse it from the response
        let detected_source = if source.is_none() {
            output
                .lines()
                .find(|line: &&str| line.starts_with("Source:"))
                .map(|line: &str| line.trim_start_matches("Source:").trim().to_string())
        } else {
            None
        };

        let translated_text = if source.is_none() {
            // Remove the "Source: ..." line from the translation
            output
                .lines()
                .filter(|line: &&str| !line.starts_with("Source:"))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        } else {
            output
        };

        Ok(serde_json::json!({
            "translated_text": translated_text,
            "source_language": source.unwrap_or_else(|| detected_source.as_deref().unwrap_or("unknown")),
            "target_language": target,
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. NotificationSendTool (contextual — needs config + encrypted store)
// ─────────────────────────────────────────────────────────────────────────────

/// Send notifications through configured channels (Telegram, webhooks).
///
/// Dispatches to the appropriate platform based on channel configuration.
pub struct NotificationSendTool {
    id: ToolId,
    dirs: aivyx_config::AivyxDirs,
    config: aivyx_config::AivyxConfig,
    master_key: aivyx_crypto::MasterKey,
}

impl NotificationSendTool {
    /// Create a new notification send tool instance.
    pub fn new(
        dirs: aivyx_config::AivyxDirs,
        config: aivyx_config::AivyxConfig,
        master_key: aivyx_crypto::MasterKey,
    ) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
            config,
            master_key,
        }
    }
}

#[async_trait]
impl Tool for NotificationSendTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "notification_send"
    }

    fn description(&self) -> &str {
        "Send a notification through configured channels (Telegram, webhook). If no channel \
         is specified, sends to all enabled channels."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The notification message to send"
                },
                "channel": {
                    "type": "string",
                    "description": "Channel name to send to (sends to all enabled channels if omitted)"
                },
                "priority": {
                    "type": "string",
                    "enum": ["low", "normal", "high"],
                    "description": "Priority level (default: normal)"
                }
            },
            "required": ["message"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("notification".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let message = input["message"].as_str().ok_or_else(|| {
            AivyxError::Agent("notification_send: missing 'message' parameter".into())
        })?;
        let channel_name = input["channel"].as_str();
        let priority = input["priority"].as_str().unwrap_or("normal");

        let channels: Vec<&aivyx_config::ChannelConfig> = if let Some(name) = channel_name {
            match self.config.find_channel(name) {
                Some(ch) => vec![ch],
                None => {
                    return Err(AivyxError::Agent(format!(
                        "notification_send: channel '{name}' not found"
                    )));
                }
            }
        } else {
            self.config.channels.iter().filter(|c| c.enabled).collect()
        };

        if channels.is_empty() {
            return Ok(serde_json::json!({
                "delivered": [],
                "message": "No channels configured or enabled",
            }));
        }

        let store = aivyx_crypto::EncryptedStore::open(self.dirs.store_path())?;
        let client = reqwest::Client::new();
        let mut results = Vec::new();

        for ch in channels {
            let result = match ch.platform {
                aivyx_config::ChannelPlatform::Telegram => {
                    send_telegram(&store, &self.master_key, &client, ch, message, priority).await
                }
                _ => {
                    // Try webhook for other platforms
                    if let Some(webhook_url) = ch.settings.get("webhook_url") {
                        send_webhook(&client, webhook_url, ch, message, priority).await
                    } else {
                        Err(AivyxError::Agent(format!(
                            "notification_send: channel '{}' ({:?}) has no webhook_url configured",
                            ch.name, ch.platform
                        )))
                    }
                }
            };

            match result {
                Ok(()) => results.push(serde_json::json!({
                    "channel": ch.name,
                    "platform": format!("{:?}", ch.platform),
                    "success": true,
                })),
                Err(e) => results.push(serde_json::json!({
                    "channel": ch.name,
                    "platform": format!("{:?}", ch.platform),
                    "success": false,
                    "error": e.to_string(),
                })),
            }
        }

        Ok(serde_json::json!({
            "delivered": results,
        }))
    }
}

/// Send a notification via Telegram Bot API.
async fn send_telegram(
    store: &aivyx_crypto::EncryptedStore,
    master_key: &aivyx_crypto::MasterKey,
    client: &reqwest::Client,
    channel: &aivyx_config::ChannelConfig,
    message: &str,
    _priority: &str,
) -> Result<()> {
    let token_ref = channel.settings.get("bot_token_ref").ok_or_else(|| {
        AivyxError::Agent(format!(
            "notification_send: Telegram channel '{}' missing 'bot_token_ref' setting",
            channel.name
        ))
    })?;

    let token_bytes = store.get(token_ref, master_key)?.ok_or_else(|| {
        AivyxError::Agent(format!(
            "notification_send: secret '{token_ref}' not found in encrypted store"
        ))
    })?;
    let token = String::from_utf8(token_bytes)
        .map_err(|e| AivyxError::Agent(format!("notification_send: invalid UTF-8 token: {e}")))?;

    let chat_id = channel.allowed_users.first().ok_or_else(|| {
        AivyxError::Agent(format!(
            "notification_send: Telegram channel '{}' has no allowed_users (needed as chat_id)",
            channel.name
        ))
    })?;

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token.trim());

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": message,
            "parse_mode": "Markdown",
        }))
        .send()
        .await
        .map_err(|e| AivyxError::Http(format!("Telegram send failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AivyxError::Agent(format!("Telegram API error: {body}")));
    }

    Ok(())
}

/// Send a notification via webhook (POST JSON).
async fn send_webhook(
    client: &reqwest::Client,
    webhook_url: &str,
    channel: &aivyx_config::ChannelConfig,
    message: &str,
    priority: &str,
) -> Result<()> {
    let resp = client
        .post(webhook_url)
        .json(&serde_json::json!({
            "channel": channel.name,
            "message": message,
            "priority": priority,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }))
        .send()
        .await
        .map_err(|e| AivyxError::Http(format!("Webhook send failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AivyxError::Agent(format!(
            "Webhook error ({}): {body}",
            channel.name
        )));
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. EmailSendTool (contextual — needs SMTP config + encrypted store)
// ─────────────────────────────────────────────────────────────────────────────

/// Send email via SMTP using the configured SMTP server.
///
/// Activates the dormant `CapabilityScope::Email` — the first tool to use it.
pub struct EmailSendTool {
    id: ToolId,
    dirs: aivyx_config::AivyxDirs,
    smtp_config: Option<aivyx_config::SmtpConfig>,
    master_key: aivyx_crypto::MasterKey,
}

impl EmailSendTool {
    /// Create a new email send tool instance.
    pub fn new(
        dirs: aivyx_config::AivyxDirs,
        smtp_config: Option<aivyx_config::SmtpConfig>,
        master_key: aivyx_crypto::MasterKey,
    ) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
            smtp_config,
            master_key,
        }
    }
}

#[async_trait]
impl Tool for EmailSendTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "email_send"
    }

    fn description(&self) -> &str {
        "Send an email via SMTP. Supports plain text and HTML body, CC, BCC, and reply-to. \
         Requires SMTP configuration in config.toml."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient email address"
                },
                "subject": {
                    "type": "string",
                    "description": "Email subject line"
                },
                "body": {
                    "type": "string",
                    "description": "Email body content"
                },
                "body_format": {
                    "type": "string",
                    "enum": ["plain", "html"],
                    "description": "Body format (default: plain)"
                },
                "cc": {
                    "type": "string",
                    "description": "CC recipient email address"
                },
                "bcc": {
                    "type": "string",
                    "description": "BCC recipient email address"
                },
                "reply_to": {
                    "type": "string",
                    "description": "Reply-To email address"
                }
            },
            "required": ["to", "subject", "body"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Email {
            allowed_recipients: vec![],
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let smtp = self.smtp_config.as_ref().ok_or_else(|| {
            AivyxError::Agent(
                "email_send: SMTP not configured. Add [smtp] section to config.toml.".into(),
            )
        })?;

        let to = input["to"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("email_send: missing 'to' parameter".into()))?;
        let subject = input["subject"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("email_send: missing 'subject' parameter".into()))?;
        let body = input["body"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("email_send: missing 'body' parameter".into()))?;
        let body_format = input["body_format"].as_str().unwrap_or("plain");

        // Parse email addresses
        let to_addr: lettre::Address = to
            .parse()
            .map_err(|e| AivyxError::Agent(format!("email_send: invalid 'to' address: {e}")))?;

        let from_addr: lettre::Address = smtp
            .from_address
            .parse()
            .map_err(|e| AivyxError::Agent(format!("email_send: invalid 'from' address: {e}")))?;

        let from_mailbox = if let Some(ref name) = smtp.from_name {
            lettre::message::Mailbox::new(Some(name.clone()), from_addr)
        } else {
            lettre::message::Mailbox::new(None, from_addr)
        };

        let to_mailbox = lettre::message::Mailbox::new(None, to_addr);

        let mut email_builder = lettre::Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(subject);

        // Optional CC
        if let Some(cc) = input["cc"].as_str() {
            let cc_addr: lettre::Address = cc
                .parse()
                .map_err(|e| AivyxError::Agent(format!("email_send: invalid 'cc' address: {e}")))?;
            email_builder = email_builder.cc(lettre::message::Mailbox::new(None, cc_addr));
        }

        // Optional BCC
        if let Some(bcc) = input["bcc"].as_str() {
            let bcc_addr: lettre::Address = bcc.parse().map_err(|e| {
                AivyxError::Agent(format!("email_send: invalid 'bcc' address: {e}"))
            })?;
            email_builder = email_builder.bcc(lettre::message::Mailbox::new(None, bcc_addr));
        }

        // Optional Reply-To
        if let Some(reply_to) = input["reply_to"].as_str() {
            let reply_addr: lettre::Address = reply_to.parse().map_err(|e| {
                AivyxError::Agent(format!("email_send: invalid 'reply_to' address: {e}"))
            })?;
            email_builder = email_builder.reply_to(lettre::message::Mailbox::new(None, reply_addr));
        }

        let email = match body_format {
            "html" => email_builder
                .header(lettre::message::header::ContentType::TEXT_HTML)
                .body(body.to_string()),
            _ => email_builder
                .header(lettre::message::header::ContentType::TEXT_PLAIN)
                .body(body.to_string()),
        }
        .map_err(|e| AivyxError::Agent(format!("email_send: failed to build email: {e}")))?;

        // Get SMTP password from encrypted store
        let store = aivyx_crypto::EncryptedStore::open(self.dirs.store_path())?;
        let password_bytes = store
            .get(&smtp.password_ref, &self.master_key)?
            .ok_or_else(|| {
                AivyxError::Agent(format!(
                    "email_send: SMTP password '{}' not found in encrypted store",
                    smtp.password_ref
                ))
            })?;
        let password = String::from_utf8(password_bytes).map_err(|e| {
            AivyxError::Agent(format!("email_send: invalid UTF-8 SMTP password: {e}"))
        })?;

        // Build SMTP transport with STARTTLS
        use lettre::AsyncTransport;
        let transport =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(&smtp.host)
                .map_err(|e| AivyxError::Agent(format!("email_send: SMTP relay error: {e}")))?
                .port(smtp.port)
                .credentials(lettre::transport::smtp::authentication::Credentials::new(
                    smtp.username.clone(),
                    password,
                ))
                .build();

        transport
            .send(email)
            .await
            .map_err(|e| AivyxError::Agent(format!("email_send: SMTP send failed: {e}")))?;

        Ok(serde_json::json!({
            "sent": true,
            "from": smtp.from_address,
            "to": to,
            "subject": subject,
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Factory
// ─────────────────────────────────────────────────────────────────────────────

/// Create the stateless network tools (http_request, web_scrape).
///
/// Contextual tools (translate, notification_send, email_send) are registered
/// separately in `session.rs` because they require runtime dependencies.
pub fn create_network_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(HttpRequestTool::new()),
        Box::new(WebScrapeTool::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_request_schema() {
        let tool = HttpRequestTool::new();
        assert_eq!(tool.name(), "http_request");
        let schema = tool.input_schema();
        assert!(schema["properties"]["url"].is_object());
        assert!(schema["properties"]["method"].is_object());
        assert!(schema["properties"]["headers"].is_object());
        assert!(schema["properties"]["auth_header"].is_object());
    }

    #[test]
    fn http_request_scope() {
        let tool = HttpRequestTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Network { .. }));
    }

    #[test]
    fn web_scrape_schema() {
        let tool = WebScrapeTool::new();
        assert_eq!(tool.name(), "web_scrape");
        let schema = tool.input_schema();
        assert!(schema["properties"]["url"].is_object());
        assert!(schema["properties"]["extract"].is_object());
        assert!(schema["properties"]["css_selector"].is_object());
    }

    #[test]
    fn web_scrape_scope() {
        let tool = WebScrapeTool::new();
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Network { .. }));
    }

    #[test]
    fn email_send_schema() {
        let tool = EmailSendTool::new(
            aivyx_config::AivyxDirs::new(std::path::PathBuf::from("/tmp/test-aivyx")),
            None,
            aivyx_crypto::MasterKey::from_bytes([0u8; 32]),
        );
        assert_eq!(tool.name(), "email_send");
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Email { .. }));
    }

    #[test]
    fn notification_send_scope() {
        let tool = NotificationSendTool::new(
            aivyx_config::AivyxDirs::new(std::path::PathBuf::from("/tmp/test-aivyx")),
            aivyx_config::AivyxConfig::default(),
            aivyx_crypto::MasterKey::from_bytes([0u8; 32]),
        );
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref s) if s == "notification"));
    }

    #[test]
    fn factory_returns_two_tools() {
        let tools = create_network_tools();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name(), "http_request");
        assert_eq!(tools[1].name(), "web_scrape");
    }

    #[test]
    fn translate_schema() {
        let tool = TranslateTool::new(
            aivyx_config::AivyxDirs::new(std::path::PathBuf::from("/tmp/test-aivyx")),
            aivyx_config::ProviderConfig::default(),
            aivyx_crypto::MasterKey::from_bytes([0u8; 32]),
        );
        assert_eq!(tool.name(), "translate");
        let schema = tool.input_schema();
        assert!(schema["properties"]["text"].is_object());
        assert!(schema["properties"]["target_language"].is_object());
    }

    #[test]
    fn extract_article_from_html() {
        let html = r#"<html><body><article><p>Hello world</p></article></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let result = extract_article(&doc, "https://example.com").unwrap();
        assert!(result["content"].as_str().unwrap().contains("Hello world"));
    }

    #[test]
    fn extract_links_from_html() {
        let html = r#"<html><body><a href="https://example.com">Example</a><a href="/about">About</a></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let result = extract_links(&doc, "https://example.com", 100).unwrap();
        assert_eq!(result["count"], 2);
    }

    #[test]
    fn extract_metadata_from_html() {
        let html = r#"<html><head><title>Test Page</title><meta property="og:title" content="OG Title"><meta name="description" content="A test"></head><body></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let result = extract_metadata(&doc, "https://example.com").unwrap();
        assert_eq!(result["title"], "Test Page");
        assert_eq!(result["og_tags"]["title"], "OG Title");
    }

    #[test]
    fn extract_tables_from_html() {
        let html = r#"<html><body><table><tr><th>Name</th><th>Age</th></tr><tr><td>Alice</td><td>30</td></tr></table></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let result = extract_tables(&doc, "https://example.com", 10).unwrap();
        assert_eq!(result["count"], 1);
        let rows = result["tables"][0]["rows"].as_array().unwrap();
        assert_eq!(rows[0]["Name"], "Alice");
        assert_eq!(rows[0]["Age"], "30");
    }
}
