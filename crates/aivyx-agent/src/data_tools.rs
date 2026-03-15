//! Data and utility tools: system time, environment, JSON parsing, and hashing.

use std::path::PathBuf;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;
use chrono::{Local, Utc};
use sha2::Digest;

/// Built-in tool: get the current date and time.
pub struct SystemTimeTool {
    id: ToolId,
}

impl Default for SystemTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTimeTool {
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
pub struct EnvReadTool {
    id: ToolId,
}

impl Default for EnvReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvReadTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// Sensitive environment variable name patterns to filter out.
///
/// Patterns are matched as substrings of the uppercased variable name.
/// `PASS` is intentionally excluded to avoid blocking `$PATH` — use
/// `PASSWORD` and `PASSWD` instead.
const SENSITIVE_ENV_PATTERNS: &[&str] = &[
    "PASSWORD",
    "PASSWD",
    "TOKEN",
    "SECRET",
    "KEY",
    "CREDENTIAL",
    "API_KEY",
    "APIKEY",
    "PASSPHRASE",
    "AUTH",
    "PRIVATE",
    "SIGNING",
    "CERTIFICATE",
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
pub struct JsonParseTool {
    id: ToolId,
}

impl Default for JsonParseTool {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonParseTool {
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Navigate a JSON value using a dot-path query.
    fn query_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
        let mut current = value;
        for segment in path.split('.') {
            if segment.is_empty() {
                continue;
            }
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
pub struct HashComputeTool {
    id: ToolId,
}

impl Default for HashComputeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HashComputeTool {
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
            "file" => {
                // Validate path against dangerous system directories.
                let validated =
                    crate::built_in_tools::resolve_and_validate_path(raw_input, "hash_compute")
                        .await?;
                tokio::fs::read(&validated).await.map_err(AivyxError::Io)?
            }
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

/// Create all data tools.
pub fn create_data_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(SystemTimeTool::new()),
        Box::new(EnvReadTool::new()),
        Box::new(JsonParseTool::new()),
        Box::new(HashComputeTool::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert_eq!(
            result["hash"],
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(result["algorithm"], "sha256");
        assert_eq!(result["mode"], "text");
    }

    #[tokio::test]
    async fn env_read_path_variable() {
        let tool = EnvReadTool::new();
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
        assert_eq!(hash.len(), 64);
        assert_eq!(result["algorithm"], "sha256");

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
        assert!(utc.contains('T'));
        let ts = result["unix_timestamp"].as_i64().unwrap();
        assert!(ts > 1_700_000_000);
    }

    #[tokio::test]
    async fn hash_compute_file_rejects_dangerous_path() {
        let tool = HashComputeTool::new();
        // `/etc` is in the dangerous paths list (exact match)
        let result = tool
            .execute(serde_json::json!({
                "input": "/etc",
                "mode": "file",
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("dangerous path"));
    }

    #[tokio::test]
    async fn hash_compute_file_validates_path() {
        let tool = HashComputeTool::new();
        // Nonexistent file should produce a validation error, not a raw IO error
        let result = tool
            .execute(serde_json::json!({
                "input": "/tmp/aivyx-nonexistent-hash-xyz",
                "mode": "file",
            }))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn expanded_sensitive_env_patterns() {
        assert!(is_sensitive_env("DATABASE_PASSWORD"));
        assert!(is_sensitive_env("GITHUB_TOKEN"));
        assert!(is_sensitive_env("AWS_SECRET_ACCESS_KEY"));
        assert!(is_sensitive_env("OAUTH_APIKEY"));
        assert!(is_sensitive_env("AUTH_HEADER"));
        assert!(is_sensitive_env("SIGNING_KEY"));
        assert!(is_sensitive_env("TLS_CERTIFICATE"));
        assert!(is_sensitive_env("DB_PASSWD"));
        // Should NOT block these
        assert!(!is_sensitive_env("HOME"));
        assert!(!is_sensitive_env("PATH"));
        assert!(!is_sensitive_env("LANG"));
        assert!(!is_sensitive_env("SHELL"));
    }
}
