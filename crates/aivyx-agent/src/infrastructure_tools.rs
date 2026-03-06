//! Phase 11D: Infrastructure, safety & advanced tools.
//!
//! Contains 7 tools split into two categories:
//! - **Stateless** (6): [`CodeExecuteTool`], [`SqlQueryTool`], [`LogAnalyzeTool`],
//!   [`ComplianceCheckTool`], [`FilePatchTool`], [`ArchiveManageTool`] — registered via
//!   [`create_infrastructure_tools()`] factory in `register_built_in_tools()`.
//! - **Contextual** (1): [`ScheduleTaskTool`] — registered per-session in `session.rs`
//!   because it needs `AivyxDirs` for config persistence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;

use crate::built_in_tools::resolve_and_validate_path;

/// Maximum output length in characters for tool results.
const MAX_TOOL_OUTPUT_CHARS: usize = 8000;

/// Maximum lines to process in log analysis (prevents OOM).
const MAX_LOG_LINES: usize = 100_000;

// ─────────────────────────────────────────────────────────────────────────────
// 1. CodeExecuteTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Execute Python or JavaScript code in a subprocess with timeout.
///
/// **Note**: This is a convenience execution wrapper, not a security sandbox.
/// OS-level sandboxing (bubblewrap/nsjail) is deferred as a future hardening
/// task. The `Custom("sandbox")` capability scope gates access at the agent level.
pub struct CodeExecuteTool {
    id: ToolId,
}

impl Default for CodeExecuteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeExecuteTool {
    /// Create a new code execute tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for CodeExecuteTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "code_execute"
    }

    fn description(&self) -> &str {
        "Execute Python or JavaScript code in a subprocess with timeout. Returns stdout, \
         stderr, exit code, and optionally reads output artifacts from the working directory. \
         Note: no OS-level sandboxing — use for trusted code only."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["python", "javascript"],
                    "description": "Programming language to execute"
                },
                "code": {
                    "type": "string",
                    "description": "Source code to execute"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Execution timeout in seconds (1-120, default: 30)"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command-line arguments to pass to the script"
                },
                "artifacts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Output file names to read back (base names only, no paths)"
                }
            },
            "required": ["language", "code"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("sandbox".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let language = input["language"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("code_execute: missing 'language'".into()))?;
        let code = input["code"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("code_execute: missing 'code'".into()))?;
        let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(30).clamp(1, 120);

        let (cmd, filename) = match language {
            "python" => ("python3", "script.py"),
            "javascript" => ("node", "script.js"),
            other => {
                return Err(AivyxError::Agent(format!(
                    "code_execute: unsupported language '{other}' (use 'python' or 'javascript')"
                )));
            }
        };

        // Create temp directory
        let sandbox_dir =
            std::env::temp_dir().join(format!("aivyx-sandbox-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&sandbox_dir).await.map_err(|e| {
            AivyxError::Agent(format!("code_execute: failed to create sandbox dir: {e}"))
        })?;

        // Write script
        let script_path = sandbox_dir.join(filename);
        tokio::fs::write(&script_path, code)
            .await
            .map_err(|e| AivyxError::Agent(format!("code_execute: failed to write script: {e}")))?;

        // Build command
        let mut command = tokio::process::Command::new(cmd);
        command.arg(filename).current_dir(&sandbox_dir);

        // Add optional args
        if let Some(args) = input["args"].as_array() {
            for arg in args {
                if let Some(a) = arg.as_str() {
                    command.arg(a);
                }
            }
        }

        let start = std::time::Instant::now();

        // Execute with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            command.output(),
        )
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        let output = match result {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                let _ = tokio::fs::remove_dir_all(&sandbox_dir).await;
                return Err(AivyxError::Agent(format!(
                    "code_execute: failed to run '{cmd}': {e}"
                )));
            }
            Err(_) => {
                let _ = tokio::fs::remove_dir_all(&sandbox_dir).await;
                return Ok(serde_json::json!({
                    "exit_code": -1,
                    "stdout": "",
                    "stderr": format!("code_execute: timeout after {timeout_secs}s"),
                    "duration_ms": duration_ms,
                    "timed_out": true,
                }));
            }
        };

        let stdout = truncate_output(&String::from_utf8_lossy(&output.stdout));
        let stderr = truncate_output(&String::from_utf8_lossy(&output.stderr));

        // Read artifacts if requested
        let artifacts = if let Some(artifact_names) = input["artifacts"].as_array() {
            let mut map = serde_json::Map::new();
            for name_val in artifact_names {
                if let Some(name) = name_val.as_str() {
                    // Security: only allow base names (no path traversal)
                    let base = Path::new(name)
                        .file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or(name);
                    let artifact_path = sandbox_dir.join(base);
                    if artifact_path.exists() {
                        match tokio::fs::read_to_string(&artifact_path).await {
                            Ok(content) => {
                                map.insert(
                                    base.to_string(),
                                    serde_json::Value::String(truncate_output(&content)),
                                );
                            }
                            Err(e) => {
                                map.insert(
                                    base.to_string(),
                                    serde_json::Value::String(format!("[read error: {e}]")),
                                );
                            }
                        }
                    }
                }
            }
            Some(serde_json::Value::Object(map))
        } else {
            None
        };

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&sandbox_dir).await;

        let mut result = serde_json::json!({
            "exit_code": output.status.code().unwrap_or(-1),
            "stdout": stdout,
            "stderr": stderr,
            "duration_ms": duration_ms,
        });
        if let Some(arts) = artifacts {
            result["artifacts"] = arts;
        }

        Ok(result)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. SqlQueryTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Read-only SQL queries against SQLite databases.
///
/// Opens databases in read-only mode. Supports query execution, table listing,
/// and schema introspection. Rejects write operations (INSERT, UPDATE, DELETE, etc.).
pub struct SqlQueryTool {
    id: ToolId,
}

impl Default for SqlQueryTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlQueryTool {
    /// Create a new SQL query tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// SQL keywords that indicate a write operation (case-insensitive).
const SQL_WRITE_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "REPLACE", "ATTACH", "DETACH",
    "VACUUM", "REINDEX",
];

#[async_trait]
impl Tool for SqlQueryTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "sql_query"
    }

    fn description(&self) -> &str {
        "Execute read-only SQL queries against SQLite databases. Supports SELECT queries, \
         table listing, and schema introspection. Write operations are rejected."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "database": {
                    "type": "string",
                    "description": "Path to the SQLite database file"
                },
                "action": {
                    "type": "string",
                    "enum": ["query", "tables", "describe"],
                    "description": "Action to perform (default: query)"
                },
                "query": {
                    "type": "string",
                    "description": "SQL query to execute (required for action=query)"
                },
                "table": {
                    "type": "string",
                    "description": "Table name (required for action=describe)"
                },
                "params": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Query parameters for parameterized queries"
                },
                "max_rows": {
                    "type": "integer",
                    "description": "Maximum rows to return (default: 1000, max: 10000)"
                }
            },
            "required": ["database"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("database".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let db_path_str = input["database"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("sql_query: missing 'database'".into()))?;
        let action = input["action"].as_str().unwrap_or("query").to_string();
        let max_rows = input["max_rows"].as_u64().unwrap_or(1000).clamp(1, 10000) as usize;
        let table_name = input["table"].as_str().map(|s| s.to_string());
        let query_str = input["query"].as_str().map(|s| s.to_string());
        let params = input["params"].clone();

        let db_path = resolve_and_validate_path(db_path_str, "sql_query").await?;

        // All rusqlite operations are blocking — wrap in spawn_blocking
        tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
            let conn = rusqlite::Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| AivyxError::Agent(format!("sql_query: failed to open database: {e}")))?;

            match action.as_str() {
                "tables" => sql_list_tables(&conn),
                "describe" => {
                    let table = table_name.as_deref().ok_or_else(|| {
                        AivyxError::Agent(
                            "sql_query: 'table' is required for action=describe".into(),
                        )
                    })?;
                    sql_describe_table(&conn, table)
                }
                _ => {
                    let query = query_str.as_deref().ok_or_else(|| {
                        AivyxError::Agent("sql_query: 'query' is required for action=query".into())
                    })?;
                    sql_execute_query(&conn, query, &params, max_rows)
                }
            }
        })
        .await
        .map_err(|e| AivyxError::Agent(format!("sql_query: task join error: {e}")))?
    }
}

fn sql_list_tables(conn: &rusqlite::Connection) -> Result<serde_json::Value> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .map_err(|e| AivyxError::Agent(format!("sql_query: {e}")))?;

    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| AivyxError::Agent(format!("sql_query: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::json!({ "tables": tables, "count": tables.len() }))
}

fn sql_describe_table(conn: &rusqlite::Connection, table: &str) -> Result<serde_json::Value> {
    // Validate table name (alphanumeric + underscore only)
    if !table.chars().all(|c: char| c.is_alphanumeric() || c == '_') {
        return Err(AivyxError::Agent(format!(
            "sql_query: invalid table name '{table}'"
        )));
    }

    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| AivyxError::Agent(format!("sql_query: {e}")))?;

    let columns: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "name": row.get::<_, String>(1)?,
                "type": row.get::<_, String>(2)?,
                "nullable": row.get::<_, i32>(3)? == 0,
                "pk": row.get::<_, i32>(5)? > 0,
            }))
        })
        .map_err(|e| AivyxError::Agent(format!("sql_query: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::json!({
        "table": table,
        "columns": columns,
    }))
}

fn sql_execute_query(
    conn: &rusqlite::Connection,
    query: &str,
    params_val: &serde_json::Value,
    max_rows: usize,
) -> Result<serde_json::Value> {
    // Reject write operations
    let trimmed = query.trim();
    let upper = trimmed.to_uppercase();
    for keyword in SQL_WRITE_KEYWORDS {
        if upper.starts_with(keyword) {
            return Err(AivyxError::Agent(format!(
                "sql_query: write operations not allowed ({keyword})"
            )));
        }
    }

    // Only allow SELECT and WITH
    if !upper.starts_with("SELECT") && !upper.starts_with("WITH") {
        return Err(AivyxError::Agent(
            "sql_query: only SELECT and WITH queries are allowed".into(),
        ));
    }

    let mut stmt = conn
        .prepare(trimmed)
        .map_err(|e| AivyxError::Agent(format!("sql_query: {e}")))?;

    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    // Build params
    let params: Vec<String> = if let Some(arr) = params_val.as_array() {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    } else {
        vec![]
    };
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();

    let mut rows_out: Vec<serde_json::Value> = Vec::new();
    let mut rows = stmt
        .query(param_refs.as_slice())
        .map_err(|e| AivyxError::Agent(format!("sql_query: {e}")))?;

    while let Some(row) = rows
        .next()
        .map_err(|e| AivyxError::Agent(format!("sql_query: {e}")))?
    {
        if rows_out.len() >= max_rows {
            break;
        }
        let mut row_map = serde_json::Map::new();
        for (i, col_name) in column_names.iter().enumerate() {
            let val: serde_json::Value = match row.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Null) => serde_json::Value::Null,
                Ok(rusqlite::types::ValueRef::Integer(n)) => serde_json::json!(n),
                Ok(rusqlite::types::ValueRef::Real(f)) => serde_json::json!(f),
                Ok(rusqlite::types::ValueRef::Text(s)) => {
                    serde_json::Value::String(String::from_utf8_lossy(s).to_string())
                }
                Ok(rusqlite::types::ValueRef::Blob(b)) => {
                    serde_json::Value::String(format!("[blob {} bytes]", b.len()))
                }
                Err(_) => serde_json::Value::Null,
            };
            row_map.insert(col_name.clone(), val);
        }
        rows_out.push(serde_json::Value::Object(row_map));
    }

    Ok(serde_json::json!({
        "columns": column_names,
        "rows": rows_out,
        "row_count": rows_out.len(),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. LogAnalyzeTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Parse and analyze structured log files (JSONL, syslog, Common Log Format).
pub struct LogAnalyzeTool {
    id: ToolId,
}

impl Default for LogAnalyzeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl LogAnalyzeTool {
    /// Create a new log analyze tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for LogAnalyzeTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "log_analyze"
    }

    fn description(&self) -> &str {
        "Parse and analyze structured log files. Supports JSONL, syslog, and Common Log \
         Format. Filter by time range, severity, and regex pattern. Compute stats including \
         severity distribution, top errors, and anomaly detection."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the log file"
                },
                "format": {
                    "type": "string",
                    "enum": ["auto", "jsonl", "syslog", "clf"],
                    "description": "Log format (default: auto-detect)"
                },
                "severity": {
                    "type": "string",
                    "description": "Filter by minimum severity (debug, info, warn, error, fatal)"
                },
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to filter log messages"
                },
                "top_n": {
                    "type": "integer",
                    "description": "Number of top error patterns to return (default: 10)"
                },
                "stats": {
                    "type": "boolean",
                    "description": "Compute statistics and anomaly detection (default: true)"
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
        let path_str = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("log_analyze: missing 'path'".into()))?;
        let path = resolve_and_validate_path(path_str, "log_analyze").await?;
        let format_hint = input["format"].as_str().unwrap_or("auto");
        let severity_filter = input["severity"].as_str();
        let pattern = input["pattern"].as_str();
        let top_n = input["top_n"].as_u64().unwrap_or(10) as usize;
        let compute_stats = input["stats"].as_bool().unwrap_or(true);

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AivyxError::Agent(format!("log_analyze: failed to read file: {e}")))?;

        let pattern_re = if let Some(p) = pattern {
            Some(
                regex::Regex::new(p)
                    .map_err(|e| AivyxError::Agent(format!("log_analyze: invalid regex: {e}")))?,
            )
        } else {
            None
        };

        let lines: Vec<&str> = content.lines().take(MAX_LOG_LINES).collect();
        let total_lines = lines.len();

        // Auto-detect format
        let detected_format = if format_hint == "auto" {
            detect_log_format(lines.first().copied().unwrap_or(""))
        } else {
            format_hint.to_string()
        };

        let mut matched = 0usize;
        let mut severity_counts: HashMap<String, usize> = HashMap::new();
        let mut error_messages: HashMap<String, usize> = HashMap::new();
        let mut hourly_errors: HashMap<String, usize> = HashMap::new();

        for line in &lines {
            let entry = parse_log_entry(line, &detected_format);

            // Severity filter
            if let Some(min_sev) = severity_filter
                && severity_level(&entry.severity) < severity_level(min_sev)
            {
                continue;
            }

            // Pattern filter
            if let Some(ref re) = pattern_re
                && !re.is_match(&entry.message)
            {
                continue;
            }

            matched += 1;

            *severity_counts.entry(entry.severity.clone()).or_default() += 1;

            // Track error patterns
            if severity_level(&entry.severity) >= severity_level("error") {
                let msg_key = if entry.message.len() > 100 {
                    entry.message[..entry.message.floor_char_boundary(100)].to_string()
                } else {
                    entry.message.clone()
                };
                *error_messages.entry(msg_key).or_default() += 1;

                // Hourly bucket for anomaly detection
                if !entry.timestamp.is_empty() {
                    let hour_key = if entry.timestamp.len() >= 13 {
                        entry.timestamp[..13].to_string()
                    } else {
                        entry.timestamp.clone()
                    };
                    *hourly_errors.entry(hour_key).or_default() += 1;
                }
            }
        }

        // Top errors
        let mut top_errors: Vec<(&String, &usize)> = error_messages.iter().collect();
        top_errors.sort_by(|a, b| b.1.cmp(a.1));
        let top_errors: Vec<serde_json::Value> = top_errors
            .iter()
            .take(top_n)
            .map(|(msg, count)| {
                serde_json::json!({
                    "message": msg,
                    "count": count,
                })
            })
            .collect();

        let mut result = serde_json::json!({
            "total_lines": total_lines,
            "matched": matched,
            "format": detected_format,
            "severity_counts": severity_counts,
            "top_errors": top_errors,
        });

        // Anomaly detection
        if compute_stats && !hourly_errors.is_empty() {
            let values: Vec<f64> = hourly_errors.values().map(|v| *v as f64).collect();
            let avg = values.iter().sum::<f64>() / values.len() as f64;
            let threshold = avg * 2.0;

            let anomalies: Vec<serde_json::Value> = hourly_errors
                .iter()
                .filter(|(_, count)| (**count as f64) > threshold)
                .map(|(hour, count)| {
                    serde_json::json!({
                        "hour": hour,
                        "error_count": count,
                        "avg_error_count": avg.round() as u64,
                    })
                })
                .collect();

            if !anomalies.is_empty() {
                result["anomalies"] = serde_json::json!(anomalies);
            }
        }

        Ok(result)
    }
}

/// A parsed log entry.
struct LogEntry {
    timestamp: String,
    severity: String,
    message: String,
}

fn detect_log_format(first_line: &str) -> String {
    if first_line.starts_with('{') && serde_json::from_str::<serde_json::Value>(first_line).is_ok()
    {
        "jsonl".to_string()
    } else if regex::Regex::new(r"^\w{3}\s+\d+\s+\d+:\d+:\d+")
        .is_ok_and(|re| re.is_match(first_line))
    {
        "syslog".to_string()
    } else if regex::Regex::new(r#"^\S+ \S+ \S+ \[.+?\] ""#).is_ok_and(|re| re.is_match(first_line))
    {
        "clf".to_string()
    } else {
        "plaintext".to_string()
    }
}

fn parse_log_entry(line: &str, format: &str) -> LogEntry {
    match format {
        "jsonl" => {
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) {
                let ts = obj
                    .get("timestamp")
                    .or_else(|| obj.get("ts"))
                    .or_else(|| obj.get("@timestamp"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sev = obj
                    .get("level")
                    .or_else(|| obj.get("severity"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("info")
                    .to_lowercase();
                let msg = obj
                    .get("msg")
                    .or_else(|| obj.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                LogEntry {
                    timestamp: ts,
                    severity: sev,
                    message: msg,
                }
            } else {
                LogEntry {
                    timestamp: String::new(),
                    severity: "info".into(),
                    message: line.to_string(),
                }
            }
        }
        "syslog" => {
            // Pattern: "Mar  6 14:30:00 hostname process[pid]: message"
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            let ts = if parts.len() >= 3 {
                parts[..3].join(" ")
            } else {
                String::new()
            };
            let message = if parts.len() >= 4 {
                parts[3].to_string()
            } else {
                line.to_string()
            };
            let severity = infer_severity_from_message(&message);
            LogEntry {
                timestamp: ts,
                severity,
                message,
            }
        }
        "clf" => {
            // Common Log Format: IP - - [timestamp] "method path proto" status size
            let message = line.to_string();
            let severity = if line.contains(" 5") {
                "error".to_string()
            } else if line.contains(" 4") {
                "warn".to_string()
            } else {
                "info".to_string()
            };
            LogEntry {
                timestamp: String::new(),
                severity,
                message,
            }
        }
        _ => LogEntry {
            timestamp: String::new(),
            severity: infer_severity_from_message(line),
            message: line.to_string(),
        },
    }
}

fn infer_severity_from_message(msg: &str) -> String {
    let lower = msg.to_lowercase();
    if lower.contains("fatal") || lower.contains("critical") || lower.contains("panic") {
        "fatal".into()
    } else if lower.contains("error") || lower.contains("err") {
        "error".into()
    } else if lower.contains("warn") {
        "warn".into()
    } else if lower.contains("debug") || lower.contains("trace") {
        "debug".into()
    } else {
        "info".into()
    }
}

fn severity_level(sev: &str) -> u8 {
    match sev.to_lowercase().as_str() {
        "trace" | "debug" => 0,
        "info" => 1,
        "warn" | "warning" => 2,
        "error" | "err" => 3,
        "fatal" | "critical" | "panic" => 4,
        _ => 1,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. ComplianceCheckTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Scan files against regex-based compliance rulesets.
pub struct ComplianceCheckTool {
    id: ToolId,
}

impl Default for ComplianceCheckTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ComplianceCheckTool {
    /// Create a new compliance check tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// A single compliance rule.
#[derive(Debug, Clone, serde::Deserialize)]
struct ComplianceRule {
    name: String,
    description: String,
    file_pattern: String,
    check_pattern: String,
    severity: String,
    remediation: String,
}

/// Built-in secrets detection rules.
fn secrets_rules() -> Vec<ComplianceRule> {
    vec![
        ComplianceRule {
            name: "aws-access-key".into(),
            description: "AWS Access Key ID".into(),
            file_pattern: "**/*".into(),
            check_pattern: r"AKIA[0-9A-Z]{16}".into(),
            severity: "critical".into(),
            remediation: "Remove AWS key and use environment variables or IAM roles".into(),
        },
        ComplianceRule {
            name: "private-key".into(),
            description: "Private key in source".into(),
            file_pattern: "**/*".into(),
            check_pattern: r"-----BEGIN (RSA |EC |DSA )?PRIVATE KEY-----".into(),
            severity: "critical".into(),
            remediation: "Remove private key from source and use a secrets manager".into(),
        },
        ComplianceRule {
            name: "generic-api-key".into(),
            description: "Generic API key pattern".into(),
            file_pattern: "**/*".into(),
            check_pattern: r#"(?i)(api[_-]?key|apikey|secret[_-]?key)\s*[=:]\s*["']?\w{16,}"#
                .into(),
            severity: "high".into(),
            remediation: "Move API keys to environment variables or encrypted store".into(),
        },
        ComplianceRule {
            name: "database-url-password".into(),
            description: "Database URL with embedded password".into(),
            file_pattern: "**/*".into(),
            check_pattern: r"(mysql|postgres|mongodb)://\w+:[^@\s]+@".into(),
            severity: "high".into(),
            remediation: "Use connection string without embedded password".into(),
        },
    ]
}

/// Built-in security rules.
fn security_rules() -> Vec<ComplianceRule> {
    vec![
        ComplianceRule {
            name: "eval-usage".into(),
            description: "Dynamic code evaluation".into(),
            file_pattern: "**/*.{py,js,ts}".into(),
            check_pattern: r"\beval\s*\(".into(),
            severity: "high".into(),
            remediation: "Avoid eval() — use safe alternatives like JSON.parse or ast.literal_eval"
                .into(),
        },
        ComplianceRule {
            name: "security-todo".into(),
            description: "Security-related TODO/FIXME".into(),
            file_pattern: "**/*".into(),
            check_pattern: r"(?i)(TODO|FIXME|HACK|XXX).*security".into(),
            severity: "medium".into(),
            remediation: "Address security-related TODO items".into(),
        },
    ]
}

#[async_trait]
impl Tool for ComplianceCheckTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "compliance_check"
    }

    fn description(&self) -> &str {
        "Scan files against compliance rulesets. Built-in rulesets: 'secrets' (AWS keys, \
         private keys, API keys, DB URLs), 'security' (eval usage, security TODOs). \
         Also supports custom TOML rulesets."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory or file to scan"
                },
                "ruleset": {
                    "type": "string",
                    "enum": ["secrets", "security"],
                    "description": "Built-in ruleset name (default: secrets)"
                },
                "rules_file": {
                    "type": "string",
                    "description": "Path to a custom TOML ruleset file"
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
        let path_str = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("compliance_check: missing 'path'".into()))?;
        let scan_path = resolve_and_validate_path(path_str, "compliance_check").await?;

        // Load rules
        let rules = if let Some(rules_file) = input["rules_file"].as_str() {
            let rules_path = resolve_and_validate_path(rules_file, "compliance_check").await?;
            let content = tokio::fs::read_to_string(&rules_path).await.map_err(|e| {
                AivyxError::Agent(format!("compliance_check: failed to read rules file: {e}"))
            })?;
            #[derive(serde::Deserialize)]
            struct RuleSet {
                rules: Vec<ComplianceRule>,
            }
            let rs: RuleSet = toml::from_str(&content).map_err(|e| {
                AivyxError::Agent(format!("compliance_check: invalid ruleset TOML: {e}"))
            })?;
            rs.rules
        } else {
            match input["ruleset"].as_str().unwrap_or("secrets") {
                "security" => security_rules(),
                _ => secrets_rules(),
            }
        };

        // Compile regexes
        let compiled: Vec<(ComplianceRule, regex::Regex, glob::Pattern)> = rules
            .into_iter()
            .filter_map(|rule| {
                let re = regex::Regex::new(&rule.check_pattern).ok()?;
                let pattern = glob::Pattern::new(&rule.file_pattern).ok()?;
                Some((rule, re, pattern))
            })
            .collect();

        let mut findings: Vec<serde_json::Value> = Vec::new();

        // Walk files
        let files = collect_files(&scan_path).await;
        for file_path in &files {
            let rel_path = file_path
                .strip_prefix(&scan_path)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();

            // Skip binary files
            if is_binary_file(file_path).await {
                continue;
            }

            let content = match tokio::fs::read_to_string(file_path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (rule, re, glob_pat) in &compiled {
                if !glob_pat.matches(&rel_path) {
                    continue;
                }

                for (line_num, line) in content.lines().enumerate() {
                    if let Some(m) = re.find(line) {
                        findings.push(serde_json::json!({
                            "rule": rule.name,
                            "description": rule.description,
                            "severity": rule.severity,
                            "file": rel_path,
                            "line": line_num + 1,
                            "match": m.as_str(),
                            "remediation": rule.remediation,
                        }));
                    }
                }
            }
        }

        let failed = findings.len();
        Ok(serde_json::json!({
            "passed": failed == 0,
            "failed": failed,
            "findings": findings,
        }))
    }
}

/// Recursively collect all file paths under a directory (or return the file itself).
async fn collect_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if path.is_file() {
        files.push(path.to_path_buf());
    } else if path.is_dir() {
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let p = entry.path();
                    if p.is_dir() {
                        // Skip hidden dirs and common artifact dirs
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !name.starts_with('.')
                            && name != "target"
                            && name != "node_modules"
                            && name != "__pycache__"
                        {
                            stack.push(p);
                        }
                    } else {
                        files.push(p);
                    }
                }
            }
        }
    }
    files
}

/// Check if a file appears to be binary (contains null bytes in first 512 bytes).
async fn is_binary_file(path: &Path) -> bool {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let check_len = bytes.len().min(512);
            bytes[..check_len].contains(&0)
        }
        Err(_) => true,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. FilePatchTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Apply unified diff patches to files.
pub struct FilePatchTool {
    id: ToolId,
}

impl Default for FilePatchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FilePatchTool {
    /// Create a new file patch tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for FilePatchTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "file_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a file. Supports dry-run mode to preview changes \
         without modifying the file."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch text"
                },
                "target_file": {
                    "type": "string",
                    "description": "Target file path (extracted from patch header if omitted)"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Preview changes without applying (default: false)"
                }
            },
            "required": ["patch"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let patch_text = input["patch"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("file_patch: missing 'patch'".into()))?;
        let dry_run = input["dry_run"].as_bool().unwrap_or(false);

        // Determine target file
        let target_path_str = if let Some(t) = input["target_file"].as_str() {
            t.to_string()
        } else {
            // Extract from patch header: +++ b/path/to/file
            extract_patch_target(patch_text).ok_or_else(|| {
                AivyxError::Agent(
                    "file_patch: could not determine target file from patch header; \
                     specify 'target_file'"
                        .into(),
                )
            })?
        };

        let target_path = resolve_and_validate_path(&target_path_str, "file_patch").await?;

        let original = tokio::fs::read_to_string(&target_path).await.map_err(|e| {
            AivyxError::Agent(format!("file_patch: failed to read target file: {e}"))
        })?;

        // Apply patch using diffy
        let patch = diffy::Patch::from_str(patch_text)
            .map_err(|e| AivyxError::Agent(format!("file_patch: failed to parse patch: {e}")))?;

        let patched = diffy::apply(&original, &patch)
            .map_err(|e| AivyxError::Agent(format!("file_patch: failed to apply patch: {e}")))?;

        let hunks_total = patch.hunks().len();

        if dry_run {
            let preview = truncate_output(&patched);
            Ok(serde_json::json!({
                "applied": false,
                "dry_run": true,
                "hunks_total": hunks_total,
                "preview": preview,
            }))
        } else {
            tokio::fs::write(&target_path, &patched)
                .await
                .map_err(|e| {
                    AivyxError::Agent(format!("file_patch: failed to write patched file: {e}"))
                })?;

            Ok(serde_json::json!({
                "applied": true,
                "hunks_total": hunks_total,
                "target_file": target_path_str,
            }))
        }
    }
}

/// Extract the target file path from a unified diff header (`+++ b/path`).
fn extract_patch_target(patch_text: &str) -> Option<String> {
    for line in patch_text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let path = rest.strip_prefix("b/").unwrap_or(rest);
            if !path.is_empty() && path != "/dev/null" {
                return Some(path.to_string());
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. ArchiveManageTool (stateless)
// ─────────────────────────────────────────────────────────────────────────────

/// Create, extract, and list archives (tar, tar.gz, zip).
pub struct ArchiveManageTool {
    id: ToolId,
}

impl Default for ArchiveManageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveManageTool {
    /// Create a new archive management tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for ArchiveManageTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "archive_manage"
    }

    fn description(&self) -> &str {
        "Create, extract, or list archives. Supports tar, tar.gz, and zip formats. \
         Auto-detects format from file extension."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["create", "extract", "list"],
                    "description": "Operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "Archive file path"
                },
                "output_path": {
                    "type": "string",
                    "description": "Output directory for extract (default: current dir)"
                },
                "format": {
                    "type": "string",
                    "enum": ["tar", "tar.gz", "zip"],
                    "description": "Archive format (auto-detected from extension if omitted)"
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files/directories to add (required for create)"
                }
            },
            "required": ["operation", "path"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let operation = input["operation"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("archive_manage: missing 'operation'".into()))?;
        let path_str = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("archive_manage: missing 'path'".into()))?;

        let format = input["format"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| detect_archive_format(path_str));

        match operation {
            "create" => {
                let archive_path = resolve_and_validate_path(path_str, "archive_manage").await?;
                let files: Vec<String> = input["files"]
                    .as_array()
                    .ok_or_else(|| {
                        AivyxError::Agent("archive_manage: 'files' is required for create".into())
                    })?
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();

                let fmt = format.clone();
                tokio::task::spawn_blocking(move || archive_create(&archive_path, &files, &fmt))
                    .await
                    .map_err(|e| AivyxError::Agent(format!("archive_manage: {e}")))?
            }
            "extract" => {
                let archive_path = resolve_and_validate_path(path_str, "archive_manage").await?;
                let output_str = input["output_path"].as_str().unwrap_or(".");
                let output_path = resolve_and_validate_path(output_str, "archive_manage").await?;

                let fmt = format.clone();
                tokio::task::spawn_blocking(move || {
                    archive_extract(&archive_path, &output_path, &fmt)
                })
                .await
                .map_err(|e| AivyxError::Agent(format!("archive_manage: {e}")))?
            }
            "list" => {
                let archive_path = resolve_and_validate_path(path_str, "archive_manage").await?;

                let fmt = format.clone();
                tokio::task::spawn_blocking(move || archive_list(&archive_path, &fmt))
                    .await
                    .map_err(|e| AivyxError::Agent(format!("archive_manage: {e}")))?
            }
            other => Err(AivyxError::Agent(format!(
                "archive_manage: unknown operation '{other}'"
            ))),
        }
    }
}

fn detect_archive_format(path: &str) -> String {
    if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
        "tar.gz".to_string()
    } else if path.ends_with(".tar") {
        "tar".to_string()
    } else if path.ends_with(".zip") {
        "zip".to_string()
    } else {
        "tar.gz".to_string() // default
    }
}

fn archive_create(
    archive_path: &Path,
    files: &[String],
    format: &str,
) -> Result<serde_json::Value> {
    let mut file_count = 0u64;

    match format {
        "zip" => {
            let file = std::fs::File::create(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to create archive: {e}"))
            })?;
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            for file_path in files {
                let path = Path::new(file_path);
                if path.is_file() {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file_path);
                    zip.start_file(name, options).map_err(|e| {
                        AivyxError::Agent(format!("archive_manage: zip error: {e}"))
                    })?;
                    let mut f = std::fs::File::open(path).map_err(|e| {
                        AivyxError::Agent(format!(
                            "archive_manage: failed to read '{file_path}': {e}"
                        ))
                    })?;
                    std::io::copy(&mut f, &mut zip).map_err(|e| {
                        AivyxError::Agent(format!("archive_manage: zip write error: {e}"))
                    })?;
                    file_count += 1;
                } else if path.is_dir() {
                    file_count += zip_add_directory(&mut zip, path, path, options)?;
                }
            }
            zip.finish().map_err(|e| {
                AivyxError::Agent(format!("archive_manage: zip finalize error: {e}"))
            })?;
        }
        "tar" => {
            let file = std::fs::File::create(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to create archive: {e}"))
            })?;
            let mut builder = tar::Builder::new(file);
            for file_path in files {
                let path = Path::new(file_path);
                if path.is_file() {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file_path);
                    builder.append_path_with_name(path, name).map_err(|e| {
                        AivyxError::Agent(format!("archive_manage: tar error: {e}"))
                    })?;
                    file_count += 1;
                } else if path.is_dir() {
                    builder
                        .append_dir_all(
                            path.file_name().and_then(|n| n.to_str()).unwrap_or("."),
                            path,
                        )
                        .map_err(|e| {
                            AivyxError::Agent(format!("archive_manage: tar error: {e}"))
                        })?;
                    file_count += 1; // approximate
                }
            }
            builder.finish().map_err(|e| {
                AivyxError::Agent(format!("archive_manage: tar finalize error: {e}"))
            })?;
        }
        _ => {
            // tar.gz
            let file = std::fs::File::create(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to create archive: {e}"))
            })?;
            let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(gz);
            for file_path in files {
                let path = Path::new(file_path);
                if path.is_file() {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file_path);
                    builder.append_path_with_name(path, name).map_err(|e| {
                        AivyxError::Agent(format!("archive_manage: tar.gz error: {e}"))
                    })?;
                    file_count += 1;
                } else if path.is_dir() {
                    builder
                        .append_dir_all(
                            path.file_name().and_then(|n| n.to_str()).unwrap_or("."),
                            path,
                        )
                        .map_err(|e| {
                            AivyxError::Agent(format!("archive_manage: tar.gz error: {e}"))
                        })?;
                    file_count += 1;
                }
            }
            builder
                .into_inner()
                .map_err(|e| {
                    AivyxError::Agent(format!("archive_manage: tar.gz finalize error: {e}"))
                })?
                .finish()
                .map_err(|e| {
                    AivyxError::Agent(format!("archive_manage: gzip finalize error: {e}"))
                })?;
        }
    }

    let total_bytes = std::fs::metadata(archive_path)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(serde_json::json!({
        "created": archive_path.display().to_string(),
        "format": format,
        "file_count": file_count,
        "total_bytes": total_bytes,
    }))
}

fn zip_add_directory(
    zip: &mut zip::ZipWriter<std::fs::File>,
    base: &Path,
    dir: &Path,
    options: zip::write::SimpleFileOptions,
) -> Result<u64> {
    let mut count = 0u64;
    for entry in std::fs::read_dir(dir)
        .map_err(|e| AivyxError::Agent(format!("archive_manage: failed to read dir: {e}")))?
    {
        let entry = entry
            .map_err(|e| AivyxError::Agent(format!("archive_manage: dir entry error: {e}")))?;
        let path = entry.path();
        let rel = path.strip_prefix(base).unwrap_or(&path);

        if path.is_file() {
            zip.start_file(rel.to_string_lossy(), options)
                .map_err(|e| AivyxError::Agent(format!("archive_manage: zip error: {e}")))?;
            let mut f = std::fs::File::open(&path)
                .map_err(|e| AivyxError::Agent(format!("archive_manage: read error: {e}")))?;
            std::io::copy(&mut f, zip)
                .map_err(|e| AivyxError::Agent(format!("archive_manage: zip copy error: {e}")))?;
            count += 1;
        } else if path.is_dir() {
            count += zip_add_directory(zip, base, &path, options)?;
        }
    }
    Ok(count)
}

fn archive_extract(
    archive_path: &Path,
    output_path: &Path,
    format: &str,
) -> Result<serde_json::Value> {
    std::fs::create_dir_all(output_path).map_err(|e| {
        AivyxError::Agent(format!("archive_manage: failed to create output dir: {e}"))
    })?;

    let file_count: u64;

    match format {
        "zip" => {
            let file = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to open archive: {e}"))
            })?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| AivyxError::Agent(format!("archive_manage: invalid zip: {e}")))?;
            file_count = archive.len() as u64;
            archive.extract(output_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: zip extract error: {e}"))
            })?;
        }
        "tar" => {
            let file = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to open archive: {e}"))
            })?;
            let mut archive = tar::Archive::new(file);
            let entries: Vec<_> = archive
                .entries()
                .map_err(|e| AivyxError::Agent(format!("archive_manage: tar error: {e}")))?
                .collect();
            file_count = entries.len() as u64;
            // Re-open to actually extract
            let file2 = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to re-open: {e}"))
            })?;
            let mut archive2 = tar::Archive::new(file2);
            archive2.unpack(output_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: tar extract error: {e}"))
            })?;
        }
        _ => {
            // tar.gz
            let file = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to open archive: {e}"))
            })?;
            let gz = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(gz);
            let entries: Vec<_> = archive
                .entries()
                .map_err(|e| AivyxError::Agent(format!("archive_manage: tar.gz error: {e}")))?
                .collect();
            file_count = entries.len() as u64;
            // Re-open
            let file2 = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to re-open: {e}"))
            })?;
            let gz2 = flate2::read::GzDecoder::new(file2);
            let mut archive2 = tar::Archive::new(gz2);
            archive2.unpack(output_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: tar.gz extract error: {e}"))
            })?;
        }
    }

    Ok(serde_json::json!({
        "extracted_to": output_path.display().to_string(),
        "format": format,
        "file_count": file_count,
    }))
}

fn archive_list(archive_path: &Path, format: &str) -> Result<serde_json::Value> {
    let mut entries_out: Vec<serde_json::Value> = Vec::new();
    let mut total_bytes: u64 = 0;

    match format {
        "zip" => {
            let file = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to open archive: {e}"))
            })?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| AivyxError::Agent(format!("archive_manage: invalid zip: {e}")))?;
            for i in 0..archive.len() {
                if let Ok(entry) = archive.by_index_raw(i) {
                    let size = entry.size();
                    entries_out.push(serde_json::json!({
                        "name": entry.name(),
                        "size": size,
                    }));
                    total_bytes += size;
                }
            }
        }
        "tar" => {
            let file = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to open archive: {e}"))
            })?;
            let mut archive = tar::Archive::new(file);
            for entry in archive
                .entries()
                .map_err(|e| AivyxError::Agent(format!("archive_manage: tar error: {e}")))?
                .flatten()
            {
                let size = entry.size();
                let name = entry
                    .path()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                entries_out.push(serde_json::json!({
                    "name": name,
                    "size": size,
                }));
                total_bytes += size;
            }
        }
        _ => {
            // tar.gz
            let file = std::fs::File::open(archive_path).map_err(|e| {
                AivyxError::Agent(format!("archive_manage: failed to open archive: {e}"))
            })?;
            let gz = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(gz);
            for entry in archive
                .entries()
                .map_err(|e| AivyxError::Agent(format!("archive_manage: tar.gz error: {e}")))?
                .flatten()
            {
                let size = entry.size();
                let name = entry
                    .path()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                entries_out.push(serde_json::json!({
                    "name": name,
                    "size": size,
                }));
                total_bytes += size;
            }
        }
    }

    Ok(serde_json::json!({
        "entries": entries_out,
        "total_files": entries_out.len(),
        "total_bytes": total_bytes,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. ScheduleTaskTool (contextual — needs AivyxDirs)
// ─────────────────────────────────────────────────────────────────────────────

/// Manage scheduled recurring tasks (list, add, remove, enable, disable).
///
/// Wraps existing `AivyxConfig` schedule CRUD. The server scheduler reloads
/// config each 60-second tick, so changes take effect without restart.
pub struct ScheduleTaskTool {
    id: ToolId,
    dirs: aivyx_config::AivyxDirs,
}

impl ScheduleTaskTool {
    /// Create a new schedule task tool instance.
    pub fn new(dirs: aivyx_config::AivyxDirs) -> Self {
        Self {
            id: ToolId::new(),
            dirs,
        }
    }
}

#[async_trait]
impl Tool for ScheduleTaskTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "schedule_task"
    }

    fn description(&self) -> &str {
        "Manage scheduled recurring tasks. Actions: 'list' (show all schedules), 'add' \
         (create new schedule with cron expression), 'remove' (delete schedule), 'enable' \
         or 'disable' (toggle schedule). Changes take effect within 60 seconds."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "add", "remove", "enable", "disable"],
                    "description": "Action to perform"
                },
                "name": {
                    "type": "string",
                    "description": "Schedule name (required for add/remove/enable/disable)"
                },
                "cron": {
                    "type": "string",
                    "description": "Cron expression (required for add, e.g. '0 7 * * *')"
                },
                "agent": {
                    "type": "string",
                    "description": "Agent profile name to run (required for add)"
                },
                "prompt": {
                    "type": "string",
                    "description": "Prompt to send to the agent (required for add)"
                },
                "notify": {
                    "type": "boolean",
                    "description": "Store result as notification (default: true)"
                }
            },
            "required": ["action"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("scheduling".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("schedule_task: missing 'action'".into()))?;

        let config_path = self.dirs.config_path();

        match action {
            "list" => {
                let config = aivyx_config::AivyxConfig::load(&config_path)?;
                let schedules: Vec<serde_json::Value> = config
                    .schedules
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "cron": s.cron,
                            "agent": s.agent,
                            "prompt": s.prompt,
                            "enabled": s.enabled,
                            "notify": s.notify,
                            "last_run_at": s.last_run_at.map(|t| t.to_rfc3339()),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({
                    "schedules": schedules,
                    "count": schedules.len(),
                }))
            }
            "add" => {
                let name = input["name"].as_str().ok_or_else(|| {
                    AivyxError::Agent("schedule_task: 'name' required for add".into())
                })?;
                let cron = input["cron"].as_str().ok_or_else(|| {
                    AivyxError::Agent("schedule_task: 'cron' required for add".into())
                })?;
                let agent = input["agent"].as_str().ok_or_else(|| {
                    AivyxError::Agent("schedule_task: 'agent' required for add".into())
                })?;
                let prompt = input["prompt"].as_str().ok_or_else(|| {
                    AivyxError::Agent("schedule_task: 'prompt' required for add".into())
                })?;

                aivyx_config::validate_cron(cron)?;

                let mut config = aivyx_config::AivyxConfig::load(&config_path)?;
                let mut entry = aivyx_config::ScheduleEntry::new(name, cron, agent, prompt);
                if let Some(notify) = input["notify"].as_bool() {
                    entry.notify = notify;
                }
                config.add_schedule(entry)?;
                config.save(&config_path)?;

                Ok(serde_json::json!({
                    "added": name,
                    "cron": cron,
                    "agent": agent,
                }))
            }
            "remove" => {
                let name = input["name"].as_str().ok_or_else(|| {
                    AivyxError::Agent("schedule_task: 'name' required for remove".into())
                })?;

                let mut config = aivyx_config::AivyxConfig::load(&config_path)?;
                config.remove_schedule(name)?;
                config.save(&config_path)?;

                Ok(serde_json::json!({ "removed": name }))
            }
            "enable" | "disable" => {
                let name = input["name"].as_str().ok_or_else(|| {
                    AivyxError::Agent(format!("schedule_task: 'name' required for {action}"))
                })?;
                let enabled = action == "enable";

                let mut config = aivyx_config::AivyxConfig::load(&config_path)?;
                let entry = config.find_schedule_mut(name).ok_or_else(|| {
                    AivyxError::Agent(format!("schedule_task: schedule '{name}' not found"))
                })?;
                entry.enabled = enabled;
                config.save(&config_path)?;

                Ok(serde_json::json!({
                    "name": name,
                    "enabled": enabled,
                }))
            }
            other => Err(AivyxError::Agent(format!(
                "schedule_task: unknown action '{other}'"
            ))),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn truncate_output(text: &str) -> String {
    if text.len() > MAX_TOOL_OUTPUT_CHARS {
        let boundary = text.floor_char_boundary(MAX_TOOL_OUTPUT_CHARS);
        format!("{}... [truncated]", &text[..boundary])
    } else {
        text.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Factory
// ─────────────────────────────────────────────────────────────────────────────

/// Create all Phase 11D infrastructure tools (stateless only).
///
/// The contextual [`ScheduleTaskTool`] is registered separately in `session.rs`.
pub fn create_infrastructure_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(CodeExecuteTool::new()),
        Box::new(SqlQueryTool::new()),
        Box::new(LogAnalyzeTool::new()),
        Box::new(ComplianceCheckTool::new()),
        Box::new(FilePatchTool::new()),
        Box::new(ArchiveManageTool::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_returns_six_tools() {
        let tools = create_infrastructure_tools();
        assert_eq!(tools.len(), 6);
        assert_eq!(tools[0].name(), "code_execute");
        assert_eq!(tools[1].name(), "sql_query");
        assert_eq!(tools[2].name(), "log_analyze");
        assert_eq!(tools[3].name(), "compliance_check");
        assert_eq!(tools[4].name(), "file_patch");
        assert_eq!(tools[5].name(), "archive_manage");
    }

    #[test]
    fn code_execute_schema() {
        let tool = CodeExecuteTool::new();
        assert_eq!(tool.name(), "code_execute");
        let schema = tool.input_schema();
        assert!(schema["properties"]["language"].is_object());
        assert!(schema["properties"]["code"].is_object());
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref s) if s == "sandbox"));
    }

    #[test]
    fn sql_query_schema() {
        let tool = SqlQueryTool::new();
        assert_eq!(tool.name(), "sql_query");
        let schema = tool.input_schema();
        assert!(schema["properties"]["database"].is_object());
        assert!(schema["properties"]["query"].is_object());
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref s) if s == "database"));
    }

    #[test]
    fn log_analyze_schema() {
        let tool = LogAnalyzeTool::new();
        assert_eq!(tool.name(), "log_analyze");
        let schema = tool.input_schema();
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["format"].is_object());
    }

    #[test]
    fn compliance_check_schema() {
        let tool = ComplianceCheckTool::new();
        assert_eq!(tool.name(), "compliance_check");
        let schema = tool.input_schema();
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["ruleset"].is_object());
    }

    #[test]
    fn file_patch_schema() {
        let tool = FilePatchTool::new();
        assert_eq!(tool.name(), "file_patch");
        let schema = tool.input_schema();
        assert!(schema["properties"]["patch"].is_object());
        assert!(schema["properties"]["dry_run"].is_object());
    }

    #[test]
    fn archive_manage_schema() {
        let tool = ArchiveManageTool::new();
        assert_eq!(tool.name(), "archive_manage");
        let schema = tool.input_schema();
        assert!(schema["properties"]["operation"].is_object());
        assert!(schema["properties"]["path"].is_object());
    }

    #[test]
    fn schedule_task_schema() {
        let tool = ScheduleTaskTool::new(aivyx_config::AivyxDirs::new("/tmp/test-aivyx"));
        assert_eq!(tool.name(), "schedule_task");
        let schema = tool.input_schema();
        assert!(schema["properties"]["action"].is_object());
        let scope = tool.required_scope().unwrap();
        assert!(matches!(scope, CapabilityScope::Custom(ref s) if s == "scheduling"));
    }

    #[test]
    fn detect_log_format_jsonl() {
        assert_eq!(
            detect_log_format(r#"{"level":"error","msg":"fail","ts":"2026-01-01T00:00:00Z"}"#),
            "jsonl"
        );
    }

    #[test]
    fn detect_log_format_plaintext() {
        assert_eq!(detect_log_format("just a regular line"), "plaintext");
    }

    #[test]
    fn severity_level_ordering() {
        assert!(severity_level("debug") < severity_level("info"));
        assert!(severity_level("info") < severity_level("warn"));
        assert!(severity_level("warn") < severity_level("error"));
        assert!(severity_level("error") < severity_level("fatal"));
    }

    #[test]
    fn extract_patch_target_from_header() {
        let patch = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new";
        assert_eq!(extract_patch_target(patch), Some("src/main.rs".to_string()));
    }

    #[test]
    fn detect_archive_format_extensions() {
        assert_eq!(detect_archive_format("file.tar.gz"), "tar.gz");
        assert_eq!(detect_archive_format("file.tgz"), "tar.gz");
        assert_eq!(detect_archive_format("file.tar"), "tar");
        assert_eq!(detect_archive_format("file.zip"), "zip");
    }

    #[test]
    fn secrets_rules_compile() {
        for rule in secrets_rules() {
            assert!(
                regex::Regex::new(&rule.check_pattern).is_ok(),
                "Failed to compile regex for rule '{}'",
                rule.name
            );
        }
    }

    #[test]
    fn security_rules_compile() {
        for rule in security_rules() {
            assert!(
                regex::Regex::new(&rule.check_pattern).is_ok(),
                "Failed to compile regex for rule '{}'",
                rule.name
            );
        }
    }

    #[tokio::test]
    async fn sql_query_tables() {
        let dir = std::env::temp_dir().join(format!("aivyx-sql-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");

        // Create a test database
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO users (name) VALUES ('Alice')", [])
                .unwrap();
        }

        let tool = SqlQueryTool::new();
        let result = tool
            .execute(serde_json::json!({
                "database": db_path.to_string_lossy(),
                "action": "tables",
            }))
            .await
            .unwrap();

        assert!(
            result["tables"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("users"))
        );

        // Test describe
        let result = tool
            .execute(serde_json::json!({
                "database": db_path.to_string_lossy(),
                "action": "describe",
                "table": "users",
            }))
            .await
            .unwrap();

        assert_eq!(result["columns"][0]["name"], "id");
        assert_eq!(result["columns"][1]["name"], "name");

        // Test query
        let result = tool
            .execute(serde_json::json!({
                "database": db_path.to_string_lossy(),
                "query": "SELECT * FROM users",
            }))
            .await
            .unwrap();

        assert_eq!(result["row_count"], 1);
        assert_eq!(result["rows"][0]["name"], "Alice");

        // Test write rejection
        let err = tool
            .execute(serde_json::json!({
                "database": db_path.to_string_lossy(),
                "query": "DROP TABLE users",
            }))
            .await;
        assert!(err.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn log_analyze_jsonl() {
        let dir = std::env::temp_dir().join(format!("aivyx-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("test.log");

        let content = r#"{"level":"info","msg":"started","ts":"2026-03-06T10:00:00Z"}
{"level":"error","msg":"connection failed","ts":"2026-03-06T10:01:00Z"}
{"level":"error","msg":"connection failed","ts":"2026-03-06T10:02:00Z"}
{"level":"info","msg":"recovered","ts":"2026-03-06T10:03:00Z"}
"#;
        std::fs::write(&log_path, content).unwrap();

        let tool = LogAnalyzeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "path": log_path.to_string_lossy(),
            }))
            .await
            .unwrap();

        assert_eq!(result["total_lines"], 4);
        assert_eq!(result["matched"], 4);
        assert_eq!(result["format"], "jsonl");
        assert_eq!(result["severity_counts"]["error"], 2);
        assert_eq!(result["severity_counts"]["info"], 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn archive_create_list_extract() {
        let dir = std::env::temp_dir().join(format!("aivyx-arc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Create test files
        let file1 = dir.join("hello.txt");
        let file2 = dir.join("world.txt");
        std::fs::write(&file1, "hello").unwrap();
        std::fs::write(&file2, "world").unwrap();

        let archive_path = dir.join("test.tar.gz");
        let extract_dir = dir.join("extracted");

        let tool = ArchiveManageTool::new();

        // Create
        let result = tool
            .execute(serde_json::json!({
                "operation": "create",
                "path": archive_path.to_string_lossy(),
                "files": [file1.to_string_lossy(), file2.to_string_lossy()],
            }))
            .await
            .unwrap();
        assert_eq!(result["file_count"], 2);

        // List
        let result = tool
            .execute(serde_json::json!({
                "operation": "list",
                "path": archive_path.to_string_lossy(),
            }))
            .await
            .unwrap();
        assert_eq!(result["total_files"], 2);

        // Extract
        let result = tool
            .execute(serde_json::json!({
                "operation": "extract",
                "path": archive_path.to_string_lossy(),
                "output_path": extract_dir.to_string_lossy(),
            }))
            .await
            .unwrap();
        assert_eq!(result["file_count"], 2);

        // Verify
        assert_eq!(
            std::fs::read_to_string(extract_dir.join("hello.txt")).unwrap(),
            "hello"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
