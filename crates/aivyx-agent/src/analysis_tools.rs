//! Phase 11A: Pure computation tools for the broadened Nonagon.
//!
//! These tools require zero or minimal new dependencies and provide
//! general-purpose data processing, text analysis, and computation
//! capabilities that transform the Nonagon from a code-focused team
//! into a general-purpose intelligence team.

use std::collections::HashMap;
use std::path::PathBuf;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;
use regex::Regex;

// Re-use path validation from built_in_tools
use crate::built_in_tools::resolve_and_validate_path;

// ---------------------------------------------------------------------------
// 1. ConfigParseTool — JSON/YAML/TOML parse, query, convert
// ---------------------------------------------------------------------------

/// Parse, validate, query, and convert between JSON, YAML, and TOML formats.
///
/// Extends the `json_parse` tool to handle the configuration formats used
/// in real-world projects. Supports dot-path queries and format conversion.
pub struct ConfigParseTool {
    id: ToolId,
}

impl Default for ConfigParseTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigParseTool {
    /// Create a new config parse tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Navigate a serde_json::Value using a dot-path query.
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
impl Tool for ConfigParseTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "config_parse"
    }

    fn description(&self) -> &str {
        "Parse, validate, query, and convert between JSON, YAML, and TOML configuration formats. Supports dot-path queries and format conversion."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "The configuration text to parse"
                },
                "format": {
                    "type": "string",
                    "enum": ["json", "yaml", "toml", "auto"],
                    "description": "Input format (default: 'auto' — detects from content)"
                },
                "query": {
                    "type": "string",
                    "description": "Dot-path query (e.g., 'server.port', 'database.hosts[0]')"
                },
                "convert_to": {
                    "type": "string",
                    "enum": ["json", "yaml", "toml"],
                    "description": "Convert the parsed config to this output format"
                }
            },
            "required": ["input"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["input"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("config_parse: missing 'input' parameter".into()))?;

        let format = input["format"].as_str().unwrap_or("auto");

        // Parse into serde_json::Value based on format
        let parsed: serde_json::Value = match format {
            "json" => serde_json::from_str(text)
                .map_err(|e| AivyxError::Agent(format!("config_parse: invalid JSON: {e}")))?,
            "yaml" => serde_yaml::from_str(text)
                .map_err(|e| AivyxError::Agent(format!("config_parse: invalid YAML: {e}")))?,
            "toml" => {
                let toml_val: toml::Value = toml::from_str(text)
                    .map_err(|e| AivyxError::Agent(format!("config_parse: invalid TOML: {e}")))?;
                serde_json::to_value(toml_val).map_err(|e| {
                    AivyxError::Agent(format!("config_parse: TOML conversion error: {e}"))
                })?
            }
            _ => {
                // Try JSON first, then TOML, then YAML
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                    v
                } else if let Ok(v) = toml::from_str::<toml::Value>(text) {
                    serde_json::to_value(v).map_err(|e| {
                        AivyxError::Agent(format!("config_parse: TOML conversion error: {e}"))
                    })?
                } else {
                    serde_yaml::from_str(text).map_err(|e| {
                        AivyxError::Agent(format!(
                            "config_parse: could not parse as JSON, TOML, or YAML: {e}"
                        ))
                    })?
                }
            }
        };

        // Detect which format succeeded
        let detected_format = match format {
            "auto" => {
                if serde_json::from_str::<serde_json::Value>(text).is_ok() {
                    "json"
                } else if toml::from_str::<toml::Value>(text).is_ok() {
                    "toml"
                } else {
                    "yaml"
                }
            }
            f => f,
        };

        // Apply query if provided
        let query_result = if let Some(query) = input["query"].as_str() {
            Self::query_path(&parsed, query).cloned()
        } else {
            None
        };

        // Convert to output format if requested
        let converted = if let Some(target) = input["convert_to"].as_str() {
            Some(match target {
                "json" => serde_json::to_string_pretty(&parsed).map_err(|e| {
                    AivyxError::Agent(format!("config_parse: JSON output error: {e}"))
                })?,
                "yaml" => serde_yaml::to_string(&parsed).map_err(|e| {
                    AivyxError::Agent(format!("config_parse: YAML output error: {e}"))
                })?,
                "toml" => {
                    // serde_json::Value -> toml::Value -> string
                    let toml_val: toml::Value =
                        serde_json::from_value(parsed.clone()).map_err(|e| {
                            AivyxError::Agent(format!(
                                "config_parse: cannot convert to TOML (arrays of mixed types are not supported): {e}"
                            ))
                        })?;
                    toml::to_string_pretty(&toml_val).map_err(|e| {
                        AivyxError::Agent(format!("config_parse: TOML output error: {e}"))
                    })?
                }
                _ => {
                    return Err(AivyxError::Agent(format!(
                        "config_parse: unsupported convert_to format: {target}"
                    )));
                }
            })
        } else {
            None
        };

        Ok(serde_json::json!({
            "detected_format": detected_format,
            "parsed": serde_json::to_string_pretty(&parsed).unwrap_or_default(),
            "query_result": query_result,
            "converted": converted,
        }))
    }
}

// ---------------------------------------------------------------------------
// 2. MathEvalTool — expressions, statistics, date arithmetic
// ---------------------------------------------------------------------------

/// Evaluate mathematical expressions and statistical functions.
///
/// Supports basic arithmetic, comparison operators, and statistical
/// functions (mean, median, stddev, percentile, min, max, sum, count).
/// All computation is pure Rust with no external dependencies.
pub struct MathEvalTool {
    id: ToolId,
}

impl Default for MathEvalTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MathEvalTool {
    /// Create a new math evaluation tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Parse a list of numbers from various formats: `[1,2,3]` or `1, 2, 3`.
    fn parse_number_list(s: &str) -> Result<Vec<f64>> {
        let cleaned = s.trim().trim_start_matches('[').trim_end_matches(']');
        let nums: std::result::Result<Vec<f64>, _> = cleaned
            .split(',')
            .map(|v| v.trim().parse::<f64>())
            .collect();
        nums.map_err(|e| AivyxError::Agent(format!("math_eval: invalid number in list: {e}")))
    }

    /// Evaluate a statistical function call like `mean([1,2,3])`.
    fn eval_function(name: &str, args: &str) -> Result<f64> {
        let values = Self::parse_number_list(args)?;
        if values.is_empty() {
            return Err(AivyxError::Agent(format!(
                "math_eval: {name}() requires at least one value"
            )));
        }

        match name {
            "mean" | "avg" | "average" => Ok(values.iter().sum::<f64>() / values.len() as f64),
            "median" => {
                let mut sorted = values.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let len = sorted.len();
                if len % 2 == 0 {
                    Ok((sorted[len / 2 - 1] + sorted[len / 2]) / 2.0)
                } else {
                    Ok(sorted[len / 2])
                }
            }
            "stddev" | "std" => {
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                let variance =
                    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
                Ok(variance.sqrt())
            }
            "variance" | "var" => {
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                Ok(values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64)
            }
            "sum" => Ok(values.iter().sum()),
            "count" | "len" => Ok(values.len() as f64),
            "min" => Ok(values.iter().copied().fold(f64::INFINITY, f64::min)),
            "max" => Ok(values.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
            "range" => {
                let min = values.iter().copied().fold(f64::INFINITY, f64::min);
                let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                Ok(max - min)
            }
            "percentile" => {
                // percentile(p, [values]) — first element is the percentile
                if values.len() < 2 {
                    return Err(AivyxError::Agent(
                        "math_eval: percentile() requires (p, v1, v2, ...)".into(),
                    ));
                }
                let p = values[0];
                if !(0.0..=100.0).contains(&p) {
                    return Err(AivyxError::Agent(
                        "math_eval: percentile must be between 0 and 100".into(),
                    ));
                }
                let mut data: Vec<f64> = values[1..].to_vec();
                data.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let rank = (p / 100.0) * (data.len() - 1) as f64;
                let lower = rank.floor() as usize;
                let upper = rank.ceil() as usize;
                if lower == upper {
                    Ok(data[lower])
                } else {
                    let frac = rank - lower as f64;
                    Ok(data[lower] * (1.0 - frac) + data[upper] * frac)
                }
            }
            "correlation" | "corr" => {
                // Expects pairs: [x1, y1, x2, y2, ...]
                if values.len() % 2 != 0 || values.len() < 4 {
                    return Err(AivyxError::Agent(
                        "math_eval: correlation() requires pairs [x1,y1,x2,y2,...]".into(),
                    ));
                }
                let xs: Vec<f64> = values.iter().step_by(2).copied().collect();
                let ys: Vec<f64> = values.iter().skip(1).step_by(2).copied().collect();
                let n = xs.len() as f64;
                let mean_x = xs.iter().sum::<f64>() / n;
                let mean_y = ys.iter().sum::<f64>() / n;
                let cov: f64 = xs
                    .iter()
                    .zip(ys.iter())
                    .map(|(x, y)| (x - mean_x) * (y - mean_y))
                    .sum::<f64>()
                    / n;
                let std_x = (xs.iter().map(|x| (x - mean_x).powi(2)).sum::<f64>() / n).sqrt();
                let std_y = (ys.iter().map(|y| (y - mean_y).powi(2)).sum::<f64>() / n).sqrt();
                if std_x == 0.0 || std_y == 0.0 {
                    Ok(0.0)
                } else {
                    Ok(cov / (std_x * std_y))
                }
            }
            // Single-arg math functions (also reachable via strip_prefix in parse_primary)
            "sqrt" => Ok(values[0].sqrt()),
            "abs" => Ok(values[0].abs()),
            "ln" => Ok(values[0].ln()),
            "log10" => Ok(values[0].log10()),
            "ceil" => Ok(values[0].ceil()),
            "floor" => Ok(values[0].floor()),
            "round" => Ok(values[0].round()),
            "sin" => Ok(values[0].sin()),
            "cos" => Ok(values[0].cos()),
            "tan" => Ok(values[0].tan()),
            _ => Err(AivyxError::Agent(format!(
                "math_eval: unknown function '{name}'"
            ))),
        }
    }

    /// Evaluate a simple arithmetic expression (no variables, no complex parsing).
    /// Supports: +, -, *, /, %, ^, parentheses, and function calls.
    fn eval_expression(expr: &str) -> Result<f64> {
        let expr = expr.trim();

        // Check for function calls: func_name(args)
        let func_re = Regex::new(r"^([a-z_]+)\((.+)\)$")
            .map_err(|e| AivyxError::Agent(format!("math_eval: regex error: {e}")))?;
        if let Some(caps) = func_re.captures(expr) {
            let name = &caps[1];
            let args = &caps[2];
            return Self::eval_function(name, args);
        }

        // Simple numeric literal
        if let Ok(v) = expr.parse::<f64>() {
            return Ok(v);
        }

        // Constants
        match expr.to_lowercase().as_str() {
            "pi" => return Ok(std::f64::consts::PI),
            "e" => return Ok(std::f64::consts::E),
            "tau" => return Ok(std::f64::consts::TAU),
            _ => {}
        }

        // Evaluate using a simple recursive descent parser
        Self::parse_additive(expr)
    }

    // --- Recursive descent parser for arithmetic ---

    fn parse_additive(expr: &str) -> Result<f64> {
        // Find the last + or - at top level (not inside parens)
        let mut depth = 0i32;
        let mut last_add_pos = None;
        let bytes = expr.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'+' | b'-' if depth == 0 && i > 0 => {
                    // Skip if preceded by e/E (scientific notation)
                    if i > 0 && (bytes[i - 1] == b'e' || bytes[i - 1] == b'E') {
                        continue;
                    }
                    last_add_pos = Some(i);
                }
                _ => {}
            }
        }
        if let Some(pos) = last_add_pos {
            let left = Self::parse_additive(expr[..pos].trim())?;
            let right = Self::parse_multiplicative(expr[pos + 1..].trim())?;
            return if bytes[pos] == b'+' {
                Ok(left + right)
            } else {
                Ok(left - right)
            };
        }
        Self::parse_multiplicative(expr)
    }

    fn parse_multiplicative(expr: &str) -> Result<f64> {
        let mut depth = 0i32;
        let mut last_mul_pos = None;
        let bytes = expr.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'*' | b'/' | b'%' if depth == 0 => {
                    last_mul_pos = Some(i);
                }
                _ => {}
            }
        }
        if let Some(pos) = last_mul_pos {
            let left = Self::parse_multiplicative(expr[..pos].trim())?;
            let right = Self::parse_power(expr[pos + 1..].trim())?;
            return match bytes[pos] {
                b'*' => Ok(left * right),
                b'/' => {
                    if right == 0.0 {
                        Err(AivyxError::Agent("math_eval: division by zero".into()))
                    } else {
                        Ok(left / right)
                    }
                }
                b'%' => Ok(left % right),
                _ => unreachable!(),
            };
        }
        Self::parse_power(expr)
    }

    fn parse_power(expr: &str) -> Result<f64> {
        let mut depth = 0i32;
        let bytes = expr.as_bytes();
        // Find first ^ at top level (right-associative)
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'^' if depth == 0 => {
                    let base = Self::parse_unary(expr[..i].trim())?;
                    let exp = Self::parse_power(expr[i + 1..].trim())?;
                    return Ok(base.powf(exp));
                }
                _ => {}
            }
        }
        Self::parse_unary(expr)
    }

    fn parse_unary(expr: &str) -> Result<f64> {
        let expr = expr.trim();
        if let Some(rest) = expr.strip_prefix('-') {
            return Ok(-Self::parse_primary(rest.trim())?);
        }
        if let Some(rest) = expr.strip_prefix('+') {
            return Self::parse_primary(rest.trim());
        }
        Self::parse_primary(expr)
    }

    fn parse_primary(expr: &str) -> Result<f64> {
        let expr = expr.trim();

        // Parenthesized expression
        if expr.starts_with('(') && expr.ends_with(')') {
            return Self::parse_additive(&expr[1..expr.len() - 1]);
        }

        // Function call
        let func_re = Regex::new(r"^([a-z_]+)\((.+)\)$")
            .map_err(|e| AivyxError::Agent(format!("math_eval: regex error: {e}")))?;
        if let Some(caps) = func_re.captures(expr) {
            let name = &caps[1];
            let args = &caps[2];
            return Self::eval_function(name, args);
        }

        // Single-arg math functions
        if let Some(inner) = expr.strip_prefix("sqrt(").and_then(|s| s.strip_suffix(')')) {
            return Ok(Self::eval_expression(inner)?.sqrt());
        }
        if let Some(inner) = expr.strip_prefix("abs(").and_then(|s| s.strip_suffix(')')) {
            return Ok(Self::eval_expression(inner)?.abs());
        }
        if let Some(inner) = expr.strip_prefix("ln(").and_then(|s| s.strip_suffix(')')) {
            return Ok(Self::eval_expression(inner)?.ln());
        }
        if let Some(inner) = expr
            .strip_prefix("log10(")
            .and_then(|s| s.strip_suffix(')'))
        {
            return Ok(Self::eval_expression(inner)?.log10());
        }
        if let Some(inner) = expr.strip_prefix("ceil(").and_then(|s| s.strip_suffix(')')) {
            return Ok(Self::eval_expression(inner)?.ceil());
        }
        if let Some(inner) = expr
            .strip_prefix("floor(")
            .and_then(|s| s.strip_suffix(')'))
        {
            return Ok(Self::eval_expression(inner)?.floor());
        }
        if let Some(inner) = expr
            .strip_prefix("round(")
            .and_then(|s| s.strip_suffix(')'))
        {
            return Ok(Self::eval_expression(inner)?.round());
        }

        // Constants
        match expr.to_lowercase().as_str() {
            "pi" => return Ok(std::f64::consts::PI),
            "e" => return Ok(std::f64::consts::E),
            "tau" => return Ok(std::f64::consts::TAU),
            _ => {}
        }

        // Numeric literal
        expr.parse::<f64>()
            .map_err(|_| AivyxError::Agent(format!("math_eval: cannot parse '{expr}' as number")))
    }
}

#[async_trait]
impl Tool for MathEvalTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "math_eval"
    }

    fn description(&self) -> &str {
        "Evaluate mathematical expressions and statistical functions. Supports arithmetic (+, -, *, /, %, ^), functions (sqrt, abs, ln, log10, ceil, floor, round), statistical aggregations (mean, median, stddev, variance, sum, count, min, max, range, percentile, correlation), and constants (pi, e, tau)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Mathematical expression to evaluate. Examples: '2 + 3 * 4', 'mean([1,2,3,4,5])', 'sqrt(144)', 'percentile(90, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10)'"
                }
            },
            "required": ["expression"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let expr = input["expression"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("math_eval: missing 'expression' parameter".into()))?;

        if expr.len() > 2_000 {
            return Err(AivyxError::Agent(
                "math_eval: expression exceeds 2000 character limit".into(),
            ));
        }

        // Guard against stack exhaustion from deeply nested parentheses.
        let max_depth = expr.bytes().filter(|&b| b == b'(').count();
        if max_depth > 64 {
            return Err(AivyxError::Agent(
                "math_eval: expression exceeds maximum nesting depth (64)".into(),
            ));
        }

        let result = Self::eval_expression(expr)?;

        Ok(serde_json::json!({
            "expression": expr,
            "result": result,
        }))
    }
}

// ---------------------------------------------------------------------------
// 3. CsvQueryTool — tabular data processing
// ---------------------------------------------------------------------------

/// Load and query CSV/TSV data with filtering, aggregation, sorting, and grouping.
///
/// Supports column selection, row filtering, aggregations (sum, avg, count,
/// min, max), group_by, sorting, and row limits. Pure Rust via the `csv` crate.
pub struct CsvQueryTool {
    id: ToolId,
}

impl Default for CsvQueryTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CsvQueryTool {
    /// Create a new CSV query tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for CsvQueryTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "csv_query"
    }

    fn description(&self) -> &str {
        "Load and query CSV/TSV data. Supports column selection, row filtering (column operator value), aggregations (sum, avg, count, min, max), group_by, sorting, and row limits."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "string",
                    "description": "CSV/TSV text content to process"
                },
                "delimiter": {
                    "type": "string",
                    "description": "Column delimiter (default: ',' for CSV, use '\\t' for TSV)"
                },
                "columns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Column names to select (default: all)"
                },
                "filter": {
                    "type": "string",
                    "description": "Row filter expression: 'column operator value'. Operators: ==, !=, >, <, >=, <=, contains. Example: 'age > 30'"
                },
                "sort_by": {
                    "type": "string",
                    "description": "Column name to sort by"
                },
                "sort_order": {
                    "type": "string",
                    "enum": ["asc", "desc"],
                    "description": "Sort order (default: 'asc')"
                },
                "group_by": {
                    "type": "string",
                    "description": "Column name to group by"
                },
                "aggregate": {
                    "type": "string",
                    "enum": ["sum", "avg", "count", "min", "max"],
                    "description": "Aggregation function (used with group_by)"
                },
                "aggregate_column": {
                    "type": "string",
                    "description": "Column to aggregate (used with group_by + aggregate)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of rows to return (default: 100)"
                }
            },
            "required": ["data"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let data = input["data"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("csv_query: missing 'data' parameter".into()))?;

        let delimiter = input["delimiter"]
            .as_str()
            .and_then(|s| {
                if s == "\\t" || s == "\t" {
                    Some(b'\t')
                } else {
                    s.as_bytes().first().copied()
                }
            })
            .unwrap_or(b',');

        let limit = input["limit"].as_u64().unwrap_or(100) as usize;

        // Parse CSV
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(true)
            .flexible(true)
            .from_reader(data.as_bytes());

        let headers: Vec<String> = reader
            .headers()
            .map_err(|e| AivyxError::Agent(format!("csv_query: cannot read headers: {e}")))?
            .iter()
            .map(|h| h.trim().to_string())
            .collect();

        let mut rows: Vec<Vec<String>> = Vec::new();
        for result in reader.records() {
            let record = result
                .map_err(|e| AivyxError::Agent(format!("csv_query: row parse error: {e}")))?;
            rows.push(record.iter().map(|f| f.trim().to_string()).collect());
        }

        // Apply filter
        if let Some(filter_expr) = input["filter"].as_str() {
            let (col_name, op, value) = parse_filter(filter_expr)?;
            let col_idx = headers.iter().position(|h| h == &col_name).ok_or_else(|| {
                AivyxError::Agent(format!("csv_query: filter column '{col_name}' not found"))
            })?;

            rows.retain(|row| {
                let cell = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                apply_filter(cell, &op, &value)
            });
        }

        // Group by + aggregate
        if let Some(group_col) = input["group_by"].as_str() {
            let group_idx = headers.iter().position(|h| h == group_col).ok_or_else(|| {
                AivyxError::Agent(format!(
                    "csv_query: group_by column '{group_col}' not found"
                ))
            })?;

            let agg_fn = input["aggregate"].as_str().unwrap_or("count");
            let agg_col = input["aggregate_column"].as_str();

            let agg_idx = if let Some(ac) = agg_col {
                Some(headers.iter().position(|h| h == ac).ok_or_else(|| {
                    AivyxError::Agent(format!("csv_query: aggregate_column '{ac}' not found"))
                })?)
            } else {
                None
            };

            let mut groups: HashMap<String, Vec<f64>> = HashMap::new();
            let mut group_counts: HashMap<String, usize> = HashMap::new();

            for row in &rows {
                let key = row.get(group_idx).cloned().unwrap_or_default();
                *group_counts.entry(key.clone()).or_default() += 1;

                if let Some(idx) = agg_idx {
                    let val: f64 = row.get(idx).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    groups.entry(key).or_default().push(val);
                }
            }

            let result_rows: Vec<serde_json::Value> = groups
                .iter()
                .map(|(key, values)| {
                    let agg_value = match agg_fn {
                        "sum" => values.iter().sum::<f64>(),
                        "avg" => {
                            if values.is_empty() {
                                0.0
                            } else {
                                values.iter().sum::<f64>() / values.len() as f64
                            }
                        }
                        "min" => values.iter().copied().fold(f64::INFINITY, f64::min),
                        "max" => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                        _ => *group_counts.get(key).unwrap_or(&0) as f64,
                    };
                    serde_json::json!({
                        group_col: key,
                        format!("{agg_fn}_{}", agg_col.unwrap_or("rows")): agg_value,
                    })
                })
                .take(limit)
                .collect();

            return Ok(serde_json::json!({
                "row_count": result_rows.len(),
                "group_count": groups.len(),
                "rows": result_rows,
            }));
        }

        // Sort
        if let Some(sort_col) = input["sort_by"].as_str() {
            let sort_idx = headers.iter().position(|h| h == sort_col).ok_or_else(|| {
                AivyxError::Agent(format!("csv_query: sort column '{sort_col}' not found"))
            })?;
            let desc = input["sort_order"].as_str() == Some("desc");

            rows.sort_by(|a, b| {
                let va = a.get(sort_idx).map(|s| s.as_str()).unwrap_or("");
                let vb = b.get(sort_idx).map(|s| s.as_str()).unwrap_or("");
                // Try numeric comparison first
                let cmp = match (va.parse::<f64>(), vb.parse::<f64>()) {
                    (Ok(na), Ok(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
                    _ => va.cmp(vb),
                };
                if desc { cmp.reverse() } else { cmp }
            });
        }

        // Column selection
        let selected_headers: Vec<String>;
        let selected_indices: Vec<usize>;

        if let Some(cols) = input["columns"].as_array() {
            let col_names: Vec<String> = cols
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            selected_indices = col_names
                .iter()
                .filter_map(|name| headers.iter().position(|h| h == name))
                .collect();
            selected_headers = selected_indices
                .iter()
                .map(|&i| headers[i].clone())
                .collect();
        } else {
            selected_headers = headers.clone();
            selected_indices = (0..headers.len()).collect();
        }

        // Build output rows
        let total_rows = rows.len();
        let output_rows: Vec<serde_json::Value> = rows
            .iter()
            .take(limit)
            .map(|row| {
                let mut obj = serde_json::Map::new();
                for &idx in &selected_indices {
                    let key = &selected_headers
                        [selected_indices.iter().position(|&i| i == idx).unwrap_or(0)];
                    let val = row.get(idx).cloned().unwrap_or_default();
                    obj.insert(key.clone(), serde_json::Value::String(val));
                }
                serde_json::Value::Object(obj)
            })
            .collect();

        Ok(serde_json::json!({
            "headers": selected_headers,
            "row_count": output_rows.len(),
            "total_rows": total_rows,
            "truncated": total_rows > limit,
            "rows": output_rows,
        }))
    }
}

/// Parse a filter expression like "age > 30" into (column, operator, value).
fn parse_filter(expr: &str) -> Result<(String, String, String)> {
    let operators = [">=", "<=", "!=", "==", ">", "<", "contains"];
    for op in operators {
        if let Some(pos) = expr.find(op) {
            let col = expr[..pos].trim().to_string();
            let val = expr[pos + op.len()..].trim().to_string();
            return Ok((col, op.to_string(), val));
        }
    }
    Err(AivyxError::Agent(format!(
        "csv_query: invalid filter expression: '{expr}'. Use format: 'column operator value'"
    )))
}

/// Apply a filter comparison between a cell value and the filter value.
fn apply_filter(cell: &str, op: &str, value: &str) -> bool {
    match op {
        "==" => cell == value,
        "!=" => cell != value,
        "contains" => cell.contains(value),
        ">" | "<" | ">=" | "<=" => {
            match (cell.parse::<f64>(), value.parse::<f64>()) {
                (Ok(a), Ok(b)) => match op {
                    ">" => a > b,
                    "<" => a < b,
                    ">=" => a >= b,
                    "<=" => a <= b,
                    _ => false,
                },
                _ => match op {
                    // Fall back to string comparison
                    ">" => cell > value,
                    "<" => cell < value,
                    ">=" => cell >= value,
                    "<=" => cell <= value,
                    _ => false,
                },
            }
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// 4. EntityExtractTool — named entity recognition (regex-based)
// ---------------------------------------------------------------------------

/// Extract named entities from text using regex patterns and heuristics.
///
/// Identifies: emails, URLs, IP addresses, phone numbers, dates,
/// monetary values, and capitalized proper nouns (people/orgs/places).
/// Zero ML dependencies — pure regex.
pub struct EntityExtractTool {
    id: ToolId,
}

impl Default for EntityExtractTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityExtractTool {
    /// Create a new entity extraction tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for EntityExtractTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "entity_extract"
    }

    fn description(&self) -> &str {
        "Extract named entities from text: emails, URLs, IP addresses, phone numbers, dates, monetary values, and proper nouns (people, organizations, locations). Regex-based, zero ML dependencies."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to extract entities from"
                },
                "types": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Entity types to extract (default: all). Options: email, url, ip, phone, date, money, proper_noun"
                }
            },
            "required": ["text"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("entity_extract: missing 'text' parameter".into()))?;

        let type_filter: Option<Vec<String>> = input["types"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let should_extract = |t: &str| -> bool {
            type_filter
                .as_ref()
                .map(|f| f.iter().any(|ft| ft == t))
                .unwrap_or(true)
        };

        let mut entities: Vec<serde_json::Value> = Vec::new();

        // Email addresses
        if should_extract("email") {
            let re = Regex::new(r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}")
                .map_err(|e| AivyxError::Agent(format!("entity_extract: regex error: {e}")))?;
            for m in re.find_iter(text) {
                entities.push(serde_json::json!({
                    "type": "email",
                    "value": m.as_str(),
                    "start": m.start(),
                    "end": m.end(),
                }));
            }
        }

        // URLs
        if should_extract("url") {
            let re = Regex::new(r#"https?://[^\s<>"')\]]+"#)
                .map_err(|e| AivyxError::Agent(format!("entity_extract: regex error: {e}")))?;
            for m in re.find_iter(text) {
                entities.push(serde_json::json!({
                    "type": "url",
                    "value": m.as_str(),
                    "start": m.start(),
                    "end": m.end(),
                }));
            }
        }

        // IP addresses (IPv4)
        if should_extract("ip") {
            let re = Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b")
                .map_err(|e| AivyxError::Agent(format!("entity_extract: regex error: {e}")))?;
            for m in re.find_iter(text) {
                // Validate octets are 0-255
                let valid = m
                    .as_str()
                    .split('.')
                    .all(|oct| oct.parse::<u16>().is_ok_and(|n| n <= 255));
                if valid {
                    entities.push(serde_json::json!({
                        "type": "ip",
                        "value": m.as_str(),
                        "start": m.start(),
                        "end": m.end(),
                    }));
                }
            }
        }

        // Phone numbers
        if should_extract("phone") {
            let re =
                Regex::new(r"(?:\+?\d{1,3}[\s.-]?)?\(?\d{2,4}\)?[\s.-]?\d{3,4}[\s.-]?\d{3,4}\b")
                    .map_err(|e| AivyxError::Agent(format!("entity_extract: regex error: {e}")))?;
            for m in re.find_iter(text) {
                let digits: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
                if digits.len() >= 7 && digits.len() <= 15 {
                    entities.push(serde_json::json!({
                        "type": "phone",
                        "value": m.as_str(),
                        "start": m.start(),
                        "end": m.end(),
                    }));
                }
            }
        }

        // Dates (various formats)
        if should_extract("date") {
            let patterns = [
                r"\b\d{4}-\d{2}-\d{2}\b",       // 2024-01-15
                r"\b\d{1,2}/\d{1,2}/\d{2,4}\b", // 1/15/2024
                r"\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2},?\s+\d{4}\b", // January 15, 2024
                r"\b\d{1,2}\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4}\b", // 15 January 2024
            ];
            for pat in patterns {
                let re = Regex::new(pat)
                    .map_err(|e| AivyxError::Agent(format!("entity_extract: regex error: {e}")))?;
                for m in re.find_iter(text) {
                    entities.push(serde_json::json!({
                        "type": "date",
                        "value": m.as_str(),
                        "start": m.start(),
                        "end": m.end(),
                    }));
                }
            }
        }

        // Monetary values
        if should_extract("money") {
            let re = Regex::new(r"(?:[$\u{20AC}\u{00A3}\u{00A5}])\s?\d[\d,]*(?:\.\d{1,2})?|\d[\d,]*(?:\.\d{1,2})?\s?(?:USD|EUR|GBP|JPY|CAD|AUD)\b")
                .map_err(|e| AivyxError::Agent(format!("entity_extract: regex error: {e}")))?;
            for m in re.find_iter(text) {
                entities.push(serde_json::json!({
                    "type": "money",
                    "value": m.as_str(),
                    "start": m.start(),
                    "end": m.end(),
                }));
            }
        }

        // Proper nouns (capitalized multi-word sequences not at sentence start)
        if should_extract("proper_noun") {
            let re = Regex::new(r"\b(?:[A-Z][a-z]+(?:\s+(?:of|the|and|for|de|van|von|al|bin)\s+)?){1,4}[A-Z][a-z]+\b")
                .map_err(|e| AivyxError::Agent(format!("entity_extract: regex error: {e}")))?;
            for m in re.find_iter(text) {
                let val = m.as_str().trim();
                // Skip common sentence starters and short words
                if val.split_whitespace().count() >= 2
                    && ![
                        "The", "This", "That", "These", "Those", "There", "When", "Where",
                    ]
                    .contains(&val.split_whitespace().next().unwrap_or(""))
                {
                    entities.push(serde_json::json!({
                        "type": "proper_noun",
                        "value": val,
                        "start": m.start(),
                        "end": m.end(),
                    }));
                }
            }
        }

        // Sort by position
        entities.sort_by_key(|e| e["start"].as_u64().unwrap_or(0));

        Ok(serde_json::json!({
            "entity_count": entities.len(),
            "entities": entities,
        }))
    }
}

// ---------------------------------------------------------------------------
// 5. PiiDetectTool — PII detection and redaction
// ---------------------------------------------------------------------------

/// Detect personally identifiable information (PII) in text.
///
/// Identifies: email addresses, phone numbers, SSNs, credit card numbers,
/// IP addresses, and passport-like numbers. Supports configurable redaction
/// modes: mask, hash, or remove.
pub struct PiiDetectTool {
    id: ToolId,
}

impl Default for PiiDetectTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PiiDetectTool {
    /// Create a new PII detection tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Mask a string, keeping first and last char: "secret" -> "s****t"
    fn mask_value(s: &str) -> String {
        if s.len() <= 2 {
            return "*".repeat(s.len());
        }
        let chars: Vec<char> = s.chars().collect();
        let first = chars[0];
        let last = chars[chars.len() - 1];
        format!("{first}{}{last}", "*".repeat(chars.len() - 2))
    }

    /// Hash a string using SHA-256 (first 8 hex chars).
    fn hash_value(s: &str) -> String {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(s.as_bytes());
        format!("[SHA256:{}]", hex::encode(&hash[..4]))
    }
}

#[async_trait]
impl Tool for PiiDetectTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "pii_detect"
    }

    fn description(&self) -> &str {
        "Detect personally identifiable information (PII) in text: emails, phone numbers, SSNs, credit card numbers, IP addresses. Supports redaction modes: 'detect' (report only), 'mask' (partial mask), 'hash' (SHA-256), or 'remove'."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to scan for PII"
                },
                "mode": {
                    "type": "string",
                    "enum": ["detect", "mask", "hash", "remove"],
                    "description": "What to do with detected PII (default: 'detect')"
                }
            },
            "required": ["text"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("pii_detect: missing 'text' parameter".into()))?;

        let mode = input["mode"].as_str().unwrap_or("detect");

        let mut findings: Vec<serde_json::Value> = Vec::new();
        let mut redacted = text.to_string();

        // Pattern definitions: (type, regex, description)
        let patterns: Vec<(&str, &str, &str)> = vec![
            (
                "email",
                r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}",
                "Email address",
            ),
            ("ssn", r"\b\d{3}-\d{2}-\d{4}\b", "Social Security Number"),
            (
                "credit_card",
                r"\b(?:\d{4}[\s-]?){3}\d{4}\b",
                "Credit card number",
            ),
            (
                "phone",
                r"(?:\+?\d{1,3}[\s.-]?)?\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b",
                "Phone number",
            ),
            (
                "ip_address",
                r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b",
                "IP address",
            ),
            ("passport", r"\b[A-Z]{1,2}\d{6,9}\b", "Passport-like number"),
        ];

        // Collect all matches (process in reverse order for redaction)
        let mut all_matches: Vec<(usize, usize, String, String, String)> = Vec::new();

        for (pii_type, pattern, description) in &patterns {
            let re = Regex::new(pattern)
                .map_err(|e| AivyxError::Agent(format!("pii_detect: regex error: {e}")))?;

            for m in re.find_iter(text) {
                // Validate credit cards with Luhn check
                if *pii_type == "credit_card" {
                    let digits: String =
                        m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
                    if !luhn_check(&digits) {
                        continue;
                    }
                }

                // Validate IP octets
                if *pii_type == "ip_address" {
                    let valid = m
                        .as_str()
                        .split('.')
                        .all(|oct| oct.parse::<u16>().is_ok_and(|n| n <= 255));
                    if !valid {
                        continue;
                    }
                }

                all_matches.push((
                    m.start(),
                    m.end(),
                    pii_type.to_string(),
                    m.as_str().to_string(),
                    description.to_string(),
                ));
            }
        }

        // Sort by position and deduplicate overlapping matches
        all_matches.sort_by_key(|(start, _, _, _, _)| *start);

        for (start, end, pii_type, value, description) in &all_matches {
            findings.push(serde_json::json!({
                "type": pii_type,
                "value": value,
                "description": description,
                "start": start,
                "end": end,
            }));
        }

        // Apply redaction (process in reverse to preserve positions)
        if mode != "detect" {
            let mut sorted = all_matches.clone();
            sorted.sort_by(|a, b| b.0.cmp(&a.0)); // Reverse order

            for (start, end, _, value, _) in &sorted {
                let replacement = match mode {
                    "mask" => Self::mask_value(value),
                    "hash" => Self::hash_value(value),
                    "remove" => "[REDACTED]".to_string(),
                    _ => value.clone(),
                };
                redacted.replace_range(*start..*end, &replacement);
            }
        }

        Ok(serde_json::json!({
            "pii_found": !findings.is_empty(),
            "finding_count": findings.len(),
            "findings": findings,
            "redacted_text": if mode != "detect" { Some(&redacted) } else { None },
        }))
    }
}

/// Luhn algorithm for credit card validation.
fn luhn_check(digits: &str) -> bool {
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for ch in digits.chars().rev() {
        let Some(d) = ch.to_digit(10) else {
            return false;
        };
        let val = if double {
            let doubled = d * 2;
            if doubled > 9 { doubled - 9 } else { doubled }
        } else {
            d
        };
        sum += val;
        double = !double;
    }
    sum.is_multiple_of(10)
}

// ---------------------------------------------------------------------------
// 6. TextStatisticsTool — readability scores, word counts
// ---------------------------------------------------------------------------

/// Compute text statistics including readability scores, word counts,
/// and reading time estimates.
///
/// Implements Flesch-Kincaid Grade Level, Coleman-Liau Index,
/// Gunning Fog Index, and SMOG grading.
pub struct TextStatisticsTool {
    id: ToolId,
}

impl Default for TextStatisticsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextStatisticsTool {
    /// Create a new text statistics tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Count syllables in a word (English heuristic).
    fn count_syllables(word: &str) -> usize {
        let word = word.to_lowercase();
        if word.len() <= 3 {
            return 1;
        }
        let mut count = 0;
        let mut prev_vowel = false;
        let vowels = ['a', 'e', 'i', 'o', 'u', 'y'];

        for ch in word.chars() {
            let is_vowel = vowels.contains(&ch);
            if is_vowel && !prev_vowel {
                count += 1;
            }
            prev_vowel = is_vowel;
        }

        // Adjust: silent 'e' at end
        if word.ends_with('e') && count > 1 {
            count -= 1;
        }

        count.max(1)
    }
}

#[async_trait]
impl Tool for TextStatisticsTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "text_statistics"
    }

    fn description(&self) -> &str {
        "Compute text statistics: word count, sentence count, paragraph count, average sentence length, vocabulary richness, reading time, and readability scores (Flesch-Kincaid Grade Level, Coleman-Liau Index, Gunning Fog, SMOG)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to analyze"
                }
            },
            "required": ["text"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("text_statistics: missing 'text' parameter".into()))?;

        // Basic counts
        let words: Vec<&str> = text.split_whitespace().collect();
        let word_count = words.len();

        let sentences: Vec<&str> = text
            .split(['.', '!', '?'])
            .filter(|s| !s.trim().is_empty())
            .collect();
        let sentence_count = sentences.len().max(1);

        let paragraphs: Vec<&str> = text
            .split("\n\n")
            .filter(|s| !s.trim().is_empty())
            .collect();
        let paragraph_count = paragraphs.len().max(1);

        let char_count = text.chars().count();
        let letter_count = text.chars().filter(|c| c.is_alphabetic()).count();

        // Vocabulary richness
        let unique_words: std::collections::HashSet<String> =
            words.iter().map(|w| w.to_lowercase()).collect();
        let vocabulary_richness = if word_count > 0 {
            unique_words.len() as f64 / word_count as f64
        } else {
            0.0
        };

        let avg_sentence_length = word_count as f64 / sentence_count as f64;
        let avg_word_length = if word_count > 0 {
            letter_count as f64 / word_count as f64
        } else {
            0.0
        };

        // Syllable counts
        let total_syllables: usize = words.iter().map(|w| Self::count_syllables(w)).sum();
        let avg_syllables = if word_count > 0 {
            total_syllables as f64 / word_count as f64
        } else {
            0.0
        };

        // Complex words (3+ syllables)
        let complex_words = words
            .iter()
            .filter(|w| Self::count_syllables(w) >= 3)
            .count();

        // Readability scores
        // Flesch-Kincaid Grade Level
        let fk_grade = 0.39 * avg_sentence_length + 11.8 * avg_syllables - 15.59;

        // Coleman-Liau Index
        let l = letter_count as f64 / word_count.max(1) as f64 * 100.0;
        let s = sentence_count as f64 / word_count.max(1) as f64 * 100.0;
        let coleman_liau = 0.0588 * l - 0.296 * s - 15.8;

        // Gunning Fog Index
        let complex_ratio = complex_words as f64 / word_count.max(1) as f64;
        let gunning_fog = 0.4 * (avg_sentence_length + 100.0 * complex_ratio);

        // SMOG (Simple Measure of Gobbledygook)
        let smog =
            1.0430 * (complex_words as f64 * (30.0 / sentence_count.max(1) as f64)).sqrt() + 3.1291;

        // Reading time (average 238 words per minute)
        let reading_time_minutes = word_count as f64 / 238.0;

        Ok(serde_json::json!({
            "counts": {
                "words": word_count,
                "sentences": sentence_count,
                "paragraphs": paragraph_count,
                "characters": char_count,
                "letters": letter_count,
                "syllables": total_syllables,
                "complex_words": complex_words,
                "unique_words": unique_words.len(),
            },
            "averages": {
                "sentence_length": (avg_sentence_length * 10.0).round() / 10.0,
                "word_length": (avg_word_length * 10.0).round() / 10.0,
                "syllables_per_word": (avg_syllables * 100.0).round() / 100.0,
            },
            "vocabulary_richness": (vocabulary_richness * 1000.0).round() / 1000.0,
            "readability": {
                "flesch_kincaid_grade": (fk_grade * 10.0).round() / 10.0,
                "coleman_liau_index": (coleman_liau * 10.0).round() / 10.0,
                "gunning_fog_index": (gunning_fog * 10.0).round() / 10.0,
                "smog_index": (smog * 10.0).round() / 10.0,
            },
            "reading_time_minutes": (reading_time_minutes * 10.0).round() / 10.0,
        }))
    }
}

// ---------------------------------------------------------------------------
// 7. RiskMatrixTool — risk assessment framework
// ---------------------------------------------------------------------------

/// Generate risk assessments from threat/likelihood/impact tuples.
///
/// Produces a scored risk register with severity levels, prioritized
/// ranking, heat map data, and mitigation tracking.
pub struct RiskMatrixTool {
    id: ToolId,
}

impl Default for RiskMatrixTool {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskMatrixTool {
    /// Create a new risk matrix tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    /// Convert a numeric score (1-25) to a severity label.
    fn severity_label(score: f64) -> &'static str {
        if score >= 15.0 {
            "critical"
        } else if score >= 10.0 {
            "high"
        } else if score >= 5.0 {
            "medium"
        } else {
            "low"
        }
    }
}

#[async_trait]
impl Tool for RiskMatrixTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "risk_matrix"
    }

    fn description(&self) -> &str {
        "Generate risk assessments from threats with likelihood and impact scores. Produces a scored risk register with severity levels, prioritized ranking, and summary statistics."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "risks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "threat": { "type": "string", "description": "Description of the threat" },
                            "likelihood": { "type": "number", "description": "Likelihood score (1-5)" },
                            "impact": { "type": "number", "description": "Impact score (1-5)" },
                            "category": { "type": "string", "description": "Risk category (e.g., security, operational, financial)" },
                            "mitigation": { "type": "string", "description": "Proposed mitigation strategy" }
                        },
                        "required": ["threat", "likelihood", "impact"]
                    },
                    "description": "Array of risk items to assess"
                }
            },
            "required": ["risks"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let risks = input["risks"]
            .as_array()
            .ok_or_else(|| AivyxError::Agent("risk_matrix: missing 'risks' array".into()))?;

        let mut register: Vec<serde_json::Value> = Vec::new();
        let mut severity_counts: HashMap<&str, usize> = HashMap::new();

        for (i, risk) in risks.iter().enumerate() {
            let threat = risk["threat"].as_str().unwrap_or("Unknown threat");
            let likelihood = risk["likelihood"].as_f64().unwrap_or(1.0).clamp(1.0, 5.0);
            let impact = risk["impact"].as_f64().unwrap_or(1.0).clamp(1.0, 5.0);
            let category = risk["category"].as_str().unwrap_or("general");
            let mitigation = risk["mitigation"].as_str();

            let score = likelihood * impact;
            let severity = Self::severity_label(score);

            *severity_counts.entry(severity).or_default() += 1;

            register.push(serde_json::json!({
                "id": i + 1,
                "threat": threat,
                "likelihood": likelihood,
                "impact": impact,
                "score": score,
                "severity": severity,
                "category": category,
                "mitigation": mitigation,
                "residual_risk": if mitigation.is_some() { "mitigated" } else { "unmitigated" },
            }));
        }

        // Sort by score descending (highest risk first)
        register.sort_by(|a, b| {
            b["score"]
                .as_f64()
                .unwrap_or(0.0)
                .partial_cmp(&a["score"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let total_score: f64 = register
            .iter()
            .map(|r| r["score"].as_f64().unwrap_or(0.0))
            .sum();
        let avg_score = if register.is_empty() {
            0.0
        } else {
            total_score / register.len() as f64
        };
        let max_score = register
            .first()
            .and_then(|r| r["score"].as_f64())
            .unwrap_or(0.0);

        Ok(serde_json::json!({
            "risk_count": register.len(),
            "register": register,
            "summary": {
                "total_score": total_score,
                "average_score": (avg_score * 10.0).round() / 10.0,
                "max_score": max_score,
                "overall_severity": Self::severity_label(avg_score),
                "severity_distribution": severity_counts,
                "unmitigated_count": register.iter().filter(|r| r["residual_risk"] == "unmitigated").count(),
            },
        }))
    }
}

// ---------------------------------------------------------------------------
// 8. RegexReplaceTool — find-and-replace in files
// ---------------------------------------------------------------------------

/// Find and replace text in files using regex patterns.
///
/// Supports preview mode (dry run), line-range constraints, capture group
/// substitution, and reports the number of replacements made.
pub struct RegexReplaceTool {
    id: ToolId,
}

impl Default for RegexReplaceTool {
    fn default() -> Self {
        Self::new()
    }
}

impl RegexReplaceTool {
    /// Create a new regex replace tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for RegexReplaceTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "regex_replace"
    }

    fn description(&self) -> &str {
        "Find and replace text in a file using regex patterns. Supports preview mode (dry run), line-range constraints, capture group substitution ($1, $2, etc.), and reports replacement count."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to modify"
                },
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "replacement": {
                    "type": "string",
                    "description": "Replacement string (supports capture groups: $1, $2, etc.)"
                },
                "preview": {
                    "type": "boolean",
                    "description": "If true, show what would change without modifying the file (default: false)"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Only replace within lines starting from this number (1-based)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "Only replace within lines up to this number (1-based, inclusive)"
                },
                "max_replacements": {
                    "type": "integer",
                    "description": "Maximum number of replacements to make (default: unlimited)"
                }
            },
            "required": ["path", "pattern", "replacement"]
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
            .ok_or_else(|| AivyxError::Agent("regex_replace: missing 'path' parameter".into()))?;
        let pattern = input["pattern"].as_str().ok_or_else(|| {
            AivyxError::Agent("regex_replace: missing 'pattern' parameter".into())
        })?;
        let replacement = input["replacement"].as_str().ok_or_else(|| {
            AivyxError::Agent("regex_replace: missing 'replacement' parameter".into())
        })?;

        let preview = input["preview"].as_bool().unwrap_or(false);
        let start_line = input["start_line"].as_u64().map(|n| n as usize);
        let end_line = input["end_line"].as_u64().map(|n| n as usize);
        let max_replacements = input["max_replacements"].as_u64().map(|n| n as usize);

        // Validate regex (prevent ReDoS with size limit)
        if pattern.len() > 1000 {
            return Err(AivyxError::Agent(
                "regex_replace: pattern too long (max 1000 chars)".into(),
            ));
        }
        let re = Regex::new(pattern).map_err(|e| {
            AivyxError::Agent(format!("regex_replace: invalid regex '{pattern}': {e}"))
        })?;

        let canonical = resolve_and_validate_path(path, "regex_replace").await?;

        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(AivyxError::Io)?;

        let mut changes: Vec<serde_json::Value> = Vec::new();
        let mut total_replacements = 0usize;
        let mut result_lines: Vec<String> = Vec::new();

        for (line_idx, line) in content.lines().enumerate() {
            let line_num = line_idx + 1;
            let in_range =
                start_line.is_none_or(|s| line_num >= s) && end_line.is_none_or(|e| line_num <= e);

            if !in_range || max_replacements.is_some_and(|max| total_replacements >= max) {
                result_lines.push(line.to_string());
                continue;
            }

            let new_line = re.replace_all(line, replacement);
            if new_line != line {
                let match_count = re.find_iter(line).count();
                total_replacements += match_count;
                changes.push(serde_json::json!({
                    "line": line_num,
                    "before": line,
                    "after": new_line.as_ref(),
                    "matches": match_count,
                }));
            }
            result_lines.push(new_line.into_owned());
        }

        // Write back if not preview
        if !preview && !changes.is_empty() {
            let new_content = result_lines.join("\n");
            // Preserve trailing newline if original had one
            let final_content = if content.ends_with('\n') {
                format!("{new_content}\n")
            } else {
                new_content
            };
            tokio::fs::write(&canonical, final_content)
                .await
                .map_err(AivyxError::Io)?;
        }

        Ok(serde_json::json!({
            "path": canonical.to_string_lossy(),
            "pattern": pattern,
            "replacement": replacement,
            "preview": preview,
            "replacement_count": total_replacements,
            "lines_changed": changes.len(),
            "changes": changes,
            "status": if preview { "preview" } else if changes.is_empty() { "no_matches" } else { "applied" },
        }))
    }
}

// ---------------------------------------------------------------------------
// 9. HtmlToMarkdownTool — HTML to Markdown conversion
// ---------------------------------------------------------------------------

/// Convert HTML content to clean Markdown.
///
/// Preserves structure: headings, lists, tables, links, code blocks,
/// and emphasis. Uses the `html2text` crate already in the workspace.
pub struct HtmlToMarkdownTool {
    id: ToolId,
}

impl Default for HtmlToMarkdownTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HtmlToMarkdownTool {
    /// Create a new HTML to Markdown tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for HtmlToMarkdownTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "html_to_markdown"
    }

    fn description(&self) -> &str {
        "Convert HTML content to clean Markdown, preserving headings, lists, tables, links, code blocks, and emphasis."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "html": {
                    "type": "string",
                    "description": "HTML content to convert"
                },
                "width": {
                    "type": "integer",
                    "description": "Maximum line width for wrapping (default: 80)"
                }
            },
            "required": ["html"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let html = input["html"].as_str().ok_or_else(|| {
            AivyxError::Agent("html_to_markdown: missing 'html' parameter".into())
        })?;

        let width = input["width"].as_u64().unwrap_or(80) as usize;

        let markdown = html2text::from_read(html.as_bytes(), width);

        // Clean up excessive whitespace
        let cleaned: String = markdown
            .lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n");

        // Remove runs of more than 2 blank lines
        let mut result = String::new();
        let mut blank_count = 0;
        for line in cleaned.lines() {
            if line.is_empty() {
                blank_count += 1;
                if blank_count <= 2 {
                    result.push('\n');
                }
            } else {
                blank_count = 0;
                result.push_str(line);
                result.push('\n');
            }
        }

        Ok(serde_json::json!({
            "markdown": result.trim(),
            "input_length": html.len(),
            "output_length": result.trim().len(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Registration helper
// ---------------------------------------------------------------------------

/// Create all Phase 11A analysis tools as a vector of boxed trait objects.
pub fn create_analysis_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ConfigParseTool::new()),
        Box::new(MathEvalTool::new()),
        Box::new(CsvQueryTool::new()),
        Box::new(EntityExtractTool::new()),
        Box::new(PiiDetectTool::new()),
        Box::new(TextStatisticsTool::new()),
        Box::new(RiskMatrixTool::new()),
        Box::new(RegexReplaceTool::new()),
        Box::new(HtmlToMarkdownTool::new()),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ConfigParseTool tests --

    #[tokio::test]
    async fn config_parse_json() {
        let tool = ConfigParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": r#"{"name": "test", "port": 8080}"#,
                "format": "json",
                "query": "port"
            }))
            .await
            .unwrap();
        assert_eq!(result["query_result"], 8080);
        assert_eq!(result["detected_format"], "json");
    }

    #[tokio::test]
    async fn config_parse_toml() {
        let tool = ConfigParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "[server]\nport = 3000\nhost = \"localhost\"",
                "format": "toml",
                "query": "server.port"
            }))
            .await
            .unwrap();
        assert_eq!(result["query_result"], 3000);
    }

    #[tokio::test]
    async fn config_parse_yaml() {
        let tool = ConfigParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "server:\n  port: 5000\n  host: localhost",
                "format": "yaml",
                "query": "server.host"
            }))
            .await
            .unwrap();
        assert_eq!(result["query_result"], "localhost");
    }

    #[tokio::test]
    async fn config_parse_auto_detect() {
        let tool = ConfigParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": "[database]\nurl = \"postgres://localhost/db\"",
            }))
            .await
            .unwrap();
        assert_eq!(result["detected_format"], "toml");
    }

    #[tokio::test]
    async fn config_parse_convert_json_to_yaml() {
        let tool = ConfigParseTool::new();
        let result = tool
            .execute(serde_json::json!({
                "input": r#"{"key": "value"}"#,
                "format": "json",
                "convert_to": "yaml"
            }))
            .await
            .unwrap();
        assert!(result["converted"].as_str().unwrap().contains("key: value"));
    }

    // -- MathEvalTool tests --

    #[tokio::test]
    async fn math_eval_arithmetic() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "2 + 3 * 4"}))
            .await
            .unwrap();
        assert_eq!(result["result"], 14.0);
    }

    #[tokio::test]
    async fn math_eval_mean() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "mean([1, 2, 3, 4, 5])"}))
            .await
            .unwrap();
        assert_eq!(result["result"], 3.0);
    }

    #[tokio::test]
    async fn math_eval_stddev() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "stddev([2, 4, 4, 4, 5, 5, 7, 9])"}))
            .await
            .unwrap();
        let val = result["result"].as_f64().unwrap();
        assert!((val - 2.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn math_eval_sqrt() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "sqrt(144)"}))
            .await
            .unwrap();
        assert_eq!(result["result"], 12.0);
    }

    #[tokio::test]
    async fn math_eval_power() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "2 ^ 10"}))
            .await
            .unwrap();
        assert_eq!(result["result"], 1024.0);
    }

    #[tokio::test]
    async fn math_eval_division_by_zero() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "5 / 0"}))
            .await;
        assert!(result.is_err());
    }

    // -- CsvQueryTool tests --

    #[tokio::test]
    async fn csv_query_basic() {
        let tool = CsvQueryTool::new();
        let csv_data = "name,age,city\nAlice,30,NYC\nBob,25,LA\nCharlie,35,NYC";
        let result = tool
            .execute(serde_json::json!({"data": csv_data}))
            .await
            .unwrap();
        assert_eq!(result["total_rows"], 3);
        assert_eq!(result["headers"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn csv_query_filter() {
        let tool = CsvQueryTool::new();
        let csv_data = "name,age,city\nAlice,30,NYC\nBob,25,LA\nCharlie,35,NYC";
        let result = tool
            .execute(serde_json::json!({
                "data": csv_data,
                "filter": "age > 28"
            }))
            .await
            .unwrap();
        assert_eq!(result["total_rows"], 2);
    }

    #[tokio::test]
    async fn csv_query_group_by() {
        let tool = CsvQueryTool::new();
        let csv_data = "name,age,city\nAlice,30,NYC\nBob,25,LA\nCharlie,35,NYC";
        let result = tool
            .execute(serde_json::json!({
                "data": csv_data,
                "group_by": "city",
                "aggregate": "avg",
                "aggregate_column": "age"
            }))
            .await
            .unwrap();
        assert_eq!(result["group_count"], 2);
    }

    #[tokio::test]
    async fn csv_query_sort() {
        let tool = CsvQueryTool::new();
        let csv_data = "name,age\nAlice,30\nBob,25\nCharlie,35";
        let result = tool
            .execute(serde_json::json!({
                "data": csv_data,
                "sort_by": "age",
                "sort_order": "desc"
            }))
            .await
            .unwrap();
        let rows = result["rows"].as_array().unwrap();
        assert_eq!(rows[0]["name"], "Charlie");
    }

    // -- EntityExtractTool tests --

    #[tokio::test]
    async fn entity_extract_emails() {
        let tool = EntityExtractTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "Contact alice@example.com or bob@test.org for details.",
                "types": ["email"]
            }))
            .await
            .unwrap();
        assert_eq!(result["entity_count"], 2);
    }

    #[tokio::test]
    async fn entity_extract_urls() {
        let tool = EntityExtractTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "Visit https://example.com/page?q=test for more info.",
                "types": ["url"]
            }))
            .await
            .unwrap();
        assert_eq!(result["entity_count"], 1);
    }

    #[tokio::test]
    async fn entity_extract_ips() {
        let tool = EntityExtractTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "Server at 192.168.1.1, not 999.999.999.999.",
                "types": ["ip"]
            }))
            .await
            .unwrap();
        assert_eq!(result["entity_count"], 1); // 999.999 is invalid
    }

    #[tokio::test]
    async fn entity_extract_money() {
        let tool = EntityExtractTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "The total is $1,234.56 or 500 EUR.",
                "types": ["money"]
            }))
            .await
            .unwrap();
        assert!(result["entity_count"].as_u64().unwrap() >= 1);
    }

    // -- PiiDetectTool tests --

    #[tokio::test]
    async fn pii_detect_email() {
        let tool = PiiDetectTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "Send to alice@example.com please.",
                "mode": "detect"
            }))
            .await
            .unwrap();
        assert!(result["pii_found"].as_bool().unwrap());
        assert_eq!(result["finding_count"], 1);
    }

    #[tokio::test]
    async fn pii_detect_ssn() {
        let tool = PiiDetectTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "SSN: 123-45-6789",
                "mode": "mask"
            }))
            .await
            .unwrap();
        assert!(result["pii_found"].as_bool().unwrap());
        let redacted = result["redacted_text"].as_str().unwrap();
        assert!(!redacted.contains("123-45-6789"));
    }

    #[tokio::test]
    async fn pii_detect_credit_card() {
        let tool = PiiDetectTool::new();
        // Valid Luhn number
        let result = tool
            .execute(serde_json::json!({
                "text": "Card: 4111 1111 1111 1111",
                "mode": "hash"
            }))
            .await
            .unwrap();
        assert!(result["pii_found"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn pii_detect_no_pii() {
        let tool = PiiDetectTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "This text has no personal information whatsoever.",
                "mode": "detect"
            }))
            .await
            .unwrap();
        assert!(!result["pii_found"].as_bool().unwrap());
    }

    // -- TextStatisticsTool tests --

    #[tokio::test]
    async fn text_statistics_basic() {
        let tool = TextStatisticsTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "The quick brown fox jumps over the lazy dog. This is a test sentence."
            }))
            .await
            .unwrap();
        assert_eq!(result["counts"]["words"], 14);
        assert_eq!(result["counts"]["sentences"], 2);
        assert!(
            result["readability"]["flesch_kincaid_grade"]
                .as_f64()
                .is_some()
        );
        assert!(result["reading_time_minutes"].as_f64().unwrap() > 0.0);
    }

    // -- RiskMatrixTool tests --

    #[tokio::test]
    async fn risk_matrix_basic() {
        let tool = RiskMatrixTool::new();
        let result = tool
            .execute(serde_json::json!({
                "risks": [
                    {"threat": "SQL injection", "likelihood": 4, "impact": 5, "category": "security"},
                    {"threat": "Server downtime", "likelihood": 2, "impact": 3, "category": "operational"},
                    {"threat": "Data loss", "likelihood": 1, "impact": 5, "category": "security", "mitigation": "Daily backups"}
                ]
            }))
            .await
            .unwrap();
        assert_eq!(result["risk_count"], 3);
        // Highest risk should be first
        let register = result["register"].as_array().unwrap();
        assert_eq!(register[0]["threat"], "SQL injection");
        assert_eq!(register[0]["severity"], "critical");
        assert_eq!(result["summary"]["unmitigated_count"], 2);
    }

    // -- HtmlToMarkdownTool tests --

    #[tokio::test]
    async fn html_to_markdown_basic() {
        let tool = HtmlToMarkdownTool::new();
        let result = tool
            .execute(serde_json::json!({
                "html": "<h1>Title</h1><p>Hello <strong>world</strong></p>"
            }))
            .await
            .unwrap();
        let md = result["markdown"].as_str().unwrap();
        assert!(md.contains("Title"));
        assert!(md.contains("world"));
    }

    #[tokio::test]
    async fn html_to_markdown_with_links() {
        let tool = HtmlToMarkdownTool::new();
        let result = tool
            .execute(serde_json::json!({
                "html": "<p>Visit <a href=\"https://example.com\">Example</a></p>"
            }))
            .await
            .unwrap();
        let md = result["markdown"].as_str().unwrap();
        assert!(md.contains("Example"));
        assert!(md.contains("example.com"));
    }

    // -- Luhn check tests --

    #[test]
    fn luhn_valid() {
        assert!(luhn_check("4111111111111111")); // Visa test
        assert!(luhn_check("5500000000000004")); // MC test
    }

    #[test]
    fn luhn_invalid() {
        assert!(!luhn_check("1234567890123456"));
        assert!(!luhn_check("12345")); // too short
    }

    // -- Math eval edge cases --

    #[tokio::test]
    async fn math_eval_constants() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "pi"}))
            .await
            .unwrap();
        let val = result["result"].as_f64().unwrap();
        assert!((val - std::f64::consts::PI).abs() < 0.001);
    }

    #[tokio::test]
    async fn math_eval_median() {
        let tool = MathEvalTool::new();
        let result = tool
            .execute(serde_json::json!({"expression": "median([1, 3, 5, 7])"}))
            .await
            .unwrap();
        assert_eq!(result["result"], 4.0);
    }

    // -- Tool metadata tests --

    #[test]
    fn all_tools_have_names() {
        let tools = create_analysis_tools();
        assert_eq!(tools.len(), 9);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"config_parse"));
        assert!(names.contains(&"math_eval"));
        assert!(names.contains(&"csv_query"));
        assert!(names.contains(&"entity_extract"));
        assert!(names.contains(&"pii_detect"));
        assert!(names.contains(&"text_statistics"));
        assert!(names.contains(&"risk_matrix"));
        assert!(names.contains(&"regex_replace"));
        assert!(names.contains(&"html_to_markdown"));
    }

    #[test]
    fn all_tools_have_schemas() {
        for tool in create_analysis_tools() {
            let schema = tool.input_schema();
            assert_eq!(schema["type"], "object");
            assert!(
                schema["properties"].is_object(),
                "Tool {} missing properties",
                tool.name()
            );
        }
    }

    #[tokio::test]
    async fn math_eval_rejects_deep_nesting() {
        let tool = MathEvalTool::new();
        // 100 nested parentheses — exceeds the depth limit of 64
        let deep_expr = format!("{}1{}", "(".repeat(100), ")".repeat(100));
        let result = tool
            .execute(serde_json::json!({"expression": deep_expr}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nesting depth"));
    }

    #[tokio::test]
    async fn math_eval_rejects_long_expression() {
        let tool = MathEvalTool::new();
        let long_expr = "1+".repeat(1500);
        let result = tool
            .execute(serde_json::json!({"expression": long_expr}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("2000 character"));
    }
}
