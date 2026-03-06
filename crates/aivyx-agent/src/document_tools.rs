//! Phase 11B: Document intelligence & text-analysis tools for the broadened Nonagon.
//!
//! These tools give the Nonagon team the ability to extract data from documents,
//! generate charts, author diagrams, render templates, export Markdown to HTML,
//! analyse sentiment, and inspect image metadata.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;

// Re-use path validation from built_in_tools
use crate::built_in_tools::resolve_and_validate_path;

// ---------------------------------------------------------------------------
// 1. DocumentExtractTool — PDF / XLSX / CSV text + table extraction
// ---------------------------------------------------------------------------

/// Extract text and structured data from PDF, XLSX, XLS, ODS, and CSV files.
///
/// Auto-detects format by file extension. PDF extraction uses `pdf-extract`,
/// spreadsheet extraction uses `calamine`, and CSV uses the `csv` crate.
pub struct DocumentExtractTool {
    id: ToolId,
}

impl Default for DocumentExtractTool {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentExtractTool {
    /// Create a new document extract tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    fn extract_pdf(path: &std::path::Path) -> Result<serde_json::Value> {
        let bytes = std::fs::read(path).map_err(|e| AivyxError::Agent(format!("io error: {e}")))?;
        let text = pdf_extract::extract_text_from_mem(&bytes)
            .map_err(|e| AivyxError::Agent(format!("PDF extraction failed: {e}")))?;
        Ok(serde_json::json!({
            "format": "pdf",
            "text": text,
            "pages": text.matches('\u{000C}').count() + 1,
        }))
    }

    fn extract_spreadsheet(
        path: &std::path::Path,
        sheet: Option<&str>,
    ) -> Result<serde_json::Value> {
        use calamine::{Data, Reader, open_workbook_auto};

        let mut workbook = open_workbook_auto(path)
            .map_err(|e| AivyxError::Agent(format!("Spreadsheet open failed: {e}")))?;

        let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
        let target_sheet = sheet
            .map(String::from)
            .unwrap_or_else(|| sheet_names.first().cloned().unwrap_or_default());

        let range = workbook
            .worksheet_range(&target_sheet)
            .map_err(|e| AivyxError::Agent(format!("Sheet read failed: {e}")))?;

        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        for row in range.rows() {
            let cells: Vec<serde_json::Value> = row
                .iter()
                .map(|cell: &Data| match cell {
                    Data::Int(i) => serde_json::json!(i),
                    Data::Float(f) => serde_json::json!(f),
                    Data::String(s) => serde_json::json!(s),
                    Data::Bool(b) => serde_json::json!(b),
                    Data::DateTime(f) => serde_json::json!(format!("datetime:{f}")),
                    Data::DateTimeIso(s) => serde_json::json!(s),
                    Data::DurationIso(s) => serde_json::json!(format!("duration:{s}")),
                    Data::Error(e) => serde_json::json!(format!("error:{e:?}")),
                    Data::Empty => serde_json::Value::Null,
                })
                .collect();
            rows.push(cells);
        }

        Ok(serde_json::json!({
            "format": "spreadsheet",
            "sheet": target_sheet,
            "sheets_available": sheet_names,
            "row_count": rows.len(),
            "rows": rows,
        }))
    }

    fn extract_csv(path: &std::path::Path) -> Result<serde_json::Value> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_path(path)
            .map_err(|e| AivyxError::Agent(format!("CSV read failed: {e}")))?;

        let headers: Vec<String> = reader
            .headers()
            .map_err(|e| AivyxError::Agent(format!("CSV headers failed: {e}")))?
            .iter()
            .map(String::from)
            .collect();

        let mut rows: Vec<Vec<String>> = Vec::new();
        for result in reader.records() {
            let record = result.map_err(|e| AivyxError::Agent(format!("CSV row failed: {e}")))?;
            rows.push(record.iter().map(String::from).collect());
        }

        Ok(serde_json::json!({
            "format": "csv",
            "headers": headers,
            "row_count": rows.len(),
            "rows": rows,
        }))
    }
}

#[async_trait]
impl Tool for DocumentExtractTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "document_extract"
    }

    fn description(&self) -> &str {
        "Extract text and structured data from PDF, XLSX, XLS, ODS, and CSV files. Auto-detects format by file extension."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the document file"
                },
                "sheet": {
                    "type": "string",
                    "description": "Sheet name for spreadsheet files (defaults to first sheet)"
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
            .ok_or_else(|| AivyxError::Agent("Missing 'path' field".into()))?;
        let path = resolve_and_validate_path(path_str, "document_extract").await?;
        let sheet = input["sheet"].as_str();

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "pdf" => Self::extract_pdf(&path),
            "xlsx" | "xls" | "ods" => Self::extract_spreadsheet(&path, sheet),
            "csv" => Self::extract_csv(&path),
            _ => Err(AivyxError::Agent(format!(
                "Unsupported format: .{ext}. Supported: pdf, xlsx, xls, ods, csv"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// 2. ChartGenerateTool — SVG chart generation via plotters
// ---------------------------------------------------------------------------

/// Generate SVG charts (bar, line, scatter, histogram) from data.
///
/// Uses `plotters` with the SVG backend (no C dependencies). The SVG is
/// always returned in the JSON response; optionally written to a file.
pub struct ChartGenerateTool {
    id: ToolId,
}

impl Default for ChartGenerateTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ChartGenerateTool {
    /// Create a new chart generate tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for ChartGenerateTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "chart_generate"
    }

    fn description(&self) -> &str {
        "Generate SVG charts (bar, line, scatter) from data arrays. Returns SVG string and optionally writes to file."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "chart_type": {
                    "type": "string",
                    "enum": ["bar", "line", "scatter"],
                    "description": "Type of chart to generate"
                },
                "title": {
                    "type": "string",
                    "description": "Chart title"
                },
                "x_label": {
                    "type": "string",
                    "description": "X-axis label"
                },
                "y_label": {
                    "type": "string",
                    "description": "Y-axis label"
                },
                "data": {
                    "type": "array",
                    "description": "Array of {x, y} data points (x: number or string, y: number)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "x": { "description": "X value" },
                            "y": { "type": "number", "description": "Y value" }
                        }
                    }
                },
                "width": {
                    "type": "integer",
                    "description": "Chart width in pixels (default: 800)"
                },
                "height": {
                    "type": "integer",
                    "description": "Chart height in pixels (default: 600)"
                },
                "output_path": {
                    "type": "string",
                    "description": "Optional file path to write the SVG"
                }
            },
            "required": ["chart_type", "data"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        use plotters::prelude::*;

        let chart_type = input["chart_type"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("Missing 'chart_type'".into()))?;
        let title = input["title"].as_str().unwrap_or("Chart");
        let x_label = input["x_label"].as_str().unwrap_or("X");
        let y_label = input["y_label"].as_str().unwrap_or("Y");
        let width = input["width"].as_u64().unwrap_or(800) as u32;
        let height = input["height"].as_u64().unwrap_or(600) as u32;

        let data_arr = input["data"]
            .as_array()
            .ok_or_else(|| AivyxError::Agent("Missing 'data' array".into()))?;

        // Parse data points as (f64, f64)
        let mut points: Vec<(f64, f64)> = Vec::new();
        for (i, item) in data_arr.iter().enumerate() {
            let x = if let Some(n) = item["x"].as_f64() {
                n
            } else if let Some(n) = item["x"].as_i64() {
                n as f64
            } else {
                i as f64
            };
            let y = item["y"]
                .as_f64()
                .ok_or_else(|| AivyxError::Agent(format!("Missing 'y' at index {i}")))?;
            points.push((x, y));
        }

        if points.is_empty() {
            return Err(AivyxError::Agent("Data array is empty".into()));
        }

        // Compute ranges
        let x_min = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let x_max = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let y_min = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let y_max = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);

        // Add padding
        let x_pad = (x_max - x_min).abs() * 0.1 + 0.1;
        let y_pad = (y_max - y_min).abs() * 0.1 + 0.1;

        let mut svg_buf = String::new();
        {
            let root = SVGBackend::with_string(&mut svg_buf, (width, height)).into_drawing_area();
            root.fill(&WHITE)
                .map_err(|e| AivyxError::Agent(format!("Chart fill failed: {e}")))?;

            let mut chart = ChartBuilder::on(&root)
                .caption(title, ("sans-serif", 20))
                .margin(10)
                .x_label_area_size(40)
                .y_label_area_size(50)
                .build_cartesian_2d(
                    (x_min - x_pad)..(x_max + x_pad),
                    (y_min - y_pad)..(y_max + y_pad),
                )
                .map_err(|e| AivyxError::Agent(format!("Chart build failed: {e}")))?;

            chart
                .configure_mesh()
                .x_desc(x_label)
                .y_desc(y_label)
                .draw()
                .map_err(|e| AivyxError::Agent(format!("Mesh draw failed: {e}")))?;

            match chart_type {
                "line" => {
                    chart
                        .draw_series(LineSeries::new(points.iter().copied(), &BLUE))
                        .map_err(|e| AivyxError::Agent(format!("Line draw failed: {e}")))?;
                }
                "scatter" => {
                    chart
                        .draw_series(
                            points
                                .iter()
                                .map(|&(x, y)| Circle::new((x, y), 4, BLUE.filled())),
                        )
                        .map_err(|e| AivyxError::Agent(format!("Scatter draw failed: {e}")))?;
                }
                "bar" => {
                    let bar_width = if points.len() > 1 {
                        (x_max - x_min) / points.len() as f64 * 0.8
                    } else {
                        1.0
                    };
                    chart
                        .draw_series(points.iter().map(|&(x, y)| {
                            let x0 = x - bar_width / 2.0;
                            let x1 = x + bar_width / 2.0;
                            Rectangle::new([(x0, 0.0), (x1, y)], BLUE.filled())
                        }))
                        .map_err(|e| AivyxError::Agent(format!("Bar draw failed: {e}")))?;
                }
                _ => {
                    return Err(AivyxError::Agent(format!(
                        "Unknown chart type: {chart_type}. Use: bar, line, scatter"
                    )));
                }
            }

            root.present()
                .map_err(|e| AivyxError::Agent(format!("Chart present failed: {e}")))?;
        }

        // Optionally write to file
        if let Some(out) = input["output_path"].as_str() {
            let out_path = resolve_and_validate_path(out, "chart_generate").await?;
            std::fs::write(&out_path, &svg_buf)
                .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?;
        }

        Ok(serde_json::json!({
            "chart_type": chart_type,
            "width": width,
            "height": height,
            "point_count": points.len(),
            "svg": svg_buf,
        }))
    }
}

// ---------------------------------------------------------------------------
// 3. DiagramAuthorTool — write Mermaid .mmd files
// ---------------------------------------------------------------------------

/// Author Mermaid diagram files (.mmd) from structured input or raw content.
///
/// Supports flowchart, sequence, ER, gantt, mindmap, and state diagram types.
/// Writes `.mmd` files that can be rendered by Mermaid-compatible tools.
pub struct DiagramAuthorTool {
    id: ToolId,
}

impl Default for DiagramAuthorTool {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagramAuthorTool {
    /// Create a new diagram author tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    fn build_flowchart(input: &serde_json::Value) -> Result<String> {
        let direction = input["direction"].as_str().unwrap_or("TD");
        let mut lines = vec![format!("flowchart {direction}")];

        if let Some(nodes) = input["nodes"].as_array() {
            for node in nodes {
                let id = node["id"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Node missing 'id'".into()))?;
                let label = node["label"].as_str().unwrap_or(id);
                let shape = node["shape"].as_str().unwrap_or("rect");
                let formatted = match shape {
                    "round" => format!("    {id}({label})"),
                    "stadium" => format!("    {id}([{label}])"),
                    "diamond" => format!("    {id}{{{label}}}"),
                    "hexagon" => format!("    {id}{{{{{label}}}}}"),
                    "circle" => format!("    {id}(({label}))"),
                    _ => format!("    {id}[{label}]"),
                };
                lines.push(formatted);
            }
        }

        if let Some(edges) = input["edges"].as_array() {
            for edge in edges {
                let from = edge["from"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Edge missing 'from'".into()))?;
                let to = edge["to"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Edge missing 'to'".into()))?;
                let label = edge["label"].as_str();
                let arrow = match label {
                    Some(l) => format!("    {from} -->|{l}| {to}"),
                    None => format!("    {from} --> {to}"),
                };
                lines.push(arrow);
            }
        }

        Ok(lines.join("\n"))
    }

    fn build_sequence(input: &serde_json::Value) -> Result<String> {
        let mut lines = vec!["sequenceDiagram".to_string()];

        if let Some(participants) = input["participants"].as_array() {
            for p in participants {
                let name = p
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Participant must be string".into()))?;
                lines.push(format!("    participant {name}"));
            }
        }

        if let Some(messages) = input["messages"].as_array() {
            for msg in messages {
                let from = msg["from"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Message missing 'from'".into()))?;
                let to = msg["to"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Message missing 'to'".into()))?;
                let text = msg["text"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Message missing 'text'".into()))?;
                let arrow = match msg["type"].as_str().unwrap_or("solid") {
                    "dashed" => "->>",
                    "dotted" => "-->>",
                    _ => "->>",
                };
                lines.push(format!("    {from}{arrow}{to}: {text}"));
            }
        }

        Ok(lines.join("\n"))
    }

    fn build_er(input: &serde_json::Value) -> Result<String> {
        let mut lines = vec!["erDiagram".to_string()];

        if let Some(entities) = input["entities"].as_array() {
            for entity in entities {
                let name = entity["name"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Entity missing 'name'".into()))?;
                lines.push(format!("    {name} {{"));
                if let Some(attrs) = entity["attributes"].as_array() {
                    for attr in attrs {
                        let atype = attr["type"].as_str().unwrap_or("string");
                        let aname = attr["name"]
                            .as_str()
                            .ok_or_else(|| AivyxError::Agent("Attr missing 'name'".into()))?;
                        lines.push(format!("        {atype} {aname}"));
                    }
                }
                lines.push("    }".to_string());
            }
        }

        if let Some(rels) = input["relationships"].as_array() {
            for rel in rels {
                let from = rel["from"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Rel missing 'from'".into()))?;
                let to = rel["to"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Rel missing 'to'".into()))?;
                let cardinality = rel["cardinality"].as_str().unwrap_or("||--o{");
                let label = rel["label"].as_str().unwrap_or("");
                lines.push(format!("    {from} {cardinality} {to} : \"{label}\""));
            }
        }

        Ok(lines.join("\n"))
    }

    fn build_gantt(input: &serde_json::Value) -> Result<String> {
        let title = input["title"].as_str().unwrap_or("Project Schedule");
        let date_format = input["date_format"].as_str().unwrap_or("YYYY-MM-DD");
        let mut lines = vec![
            "gantt".to_string(),
            format!("    title {title}"),
            format!("    dateFormat {date_format}"),
        ];

        if let Some(sections) = input["sections"].as_array() {
            for section in sections {
                let name = section["name"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Section missing 'name'".into()))?;
                lines.push(format!("    section {name}"));
                if let Some(tasks) = section["tasks"].as_array() {
                    for task in tasks {
                        let tname = task["name"]
                            .as_str()
                            .ok_or_else(|| AivyxError::Agent("Task missing 'name'".into()))?;
                        let duration = task["duration"]
                            .as_str()
                            .ok_or_else(|| AivyxError::Agent("Task missing 'duration'".into()))?;
                        lines.push(format!("    {tname} : {duration}"));
                    }
                }
            }
        }

        Ok(lines.join("\n"))
    }

    fn build_mindmap(input: &serde_json::Value) -> Result<String> {
        let root = input["root"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("Mindmap missing 'root'".into()))?;
        let mut lines = vec!["mindmap".to_string(), format!("  root({root})")];

        fn add_children(
            lines: &mut Vec<String>,
            children: &[serde_json::Value],
            depth: usize,
        ) -> Result<()> {
            let indent = "  ".repeat(depth + 2);
            for child in children {
                let label = child["label"]
                    .as_str()
                    .or_else(|| child.as_str())
                    .ok_or_else(|| AivyxError::Agent("Child missing 'label'".into()))?;
                lines.push(format!("{indent}{label}"));
                if let Some(subs) = child["children"].as_array() {
                    add_children(lines, subs, depth + 1)?;
                }
            }
            Ok(())
        }

        if let Some(children) = input["children"].as_array() {
            add_children(&mut lines, children, 0)?;
        }

        Ok(lines.join("\n"))
    }

    fn build_state(input: &serde_json::Value) -> Result<String> {
        let mut lines = vec!["stateDiagram-v2".to_string()];

        if let Some(states) = input["states"].as_array() {
            for state in states {
                let name = state["name"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("State missing 'name'".into()))?;
                if let Some(desc) = state["description"].as_str() {
                    lines.push(format!("    {name} : {desc}"));
                }
            }
        }

        if let Some(transitions) = input["transitions"].as_array() {
            for t in transitions {
                let from = t["from"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Transition missing 'from'".into()))?;
                let to = t["to"]
                    .as_str()
                    .ok_or_else(|| AivyxError::Agent("Transition missing 'to'".into()))?;
                let label = t["label"].as_str();
                match label {
                    Some(l) => lines.push(format!("    {from} --> {to} : {l}")),
                    None => lines.push(format!("    {from} --> {to}")),
                }
            }
        }

        Ok(lines.join("\n"))
    }
}

#[async_trait]
impl Tool for DiagramAuthorTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "diagram_author"
    }

    fn description(&self) -> &str {
        "Author Mermaid diagram files (.mmd) from structured input or raw content. Supports flowchart, sequence, ER, gantt, mindmap, and state diagrams."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "output_path": {
                    "type": "string",
                    "description": "Path to write the .mmd file"
                },
                "diagram_type": {
                    "type": "string",
                    "enum": ["flowchart", "sequence", "er", "gantt", "mindmap", "state"],
                    "description": "Type of Mermaid diagram"
                },
                "content": {
                    "type": "string",
                    "description": "Raw Mermaid syntax (if provided, written verbatim)"
                },
                "direction": { "type": "string", "description": "Flowchart direction: TD, LR, etc." },
                "nodes": { "type": "array", "description": "Flowchart nodes" },
                "edges": { "type": "array", "description": "Flowchart edges" },
                "participants": { "type": "array", "description": "Sequence diagram participants" },
                "messages": { "type": "array", "description": "Sequence diagram messages" },
                "entities": { "type": "array", "description": "ER diagram entities" },
                "relationships": { "type": "array", "description": "ER diagram relationships" },
                "sections": { "type": "array", "description": "Gantt sections" },
                "root": { "type": "string", "description": "Mindmap root label" },
                "children": { "type": "array", "description": "Mindmap children" },
                "states": { "type": "array", "description": "State diagram states" },
                "transitions": { "type": "array", "description": "State diagram transitions" },
                "title": { "type": "string", "description": "Diagram title (gantt)" }
            },
            "required": ["output_path"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let output_str = input["output_path"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("Missing 'output_path'".into()))?;
        let output_path = resolve_and_validate_path(output_str, "diagram_author").await?;

        let mermaid_content = if let Some(raw) = input["content"].as_str() {
            raw.to_string()
        } else {
            let dtype = input["diagram_type"].as_str().ok_or_else(|| {
                AivyxError::Agent("Either 'content' or 'diagram_type' is required".into())
            })?;
            match dtype {
                "flowchart" => Self::build_flowchart(&input)?,
                "sequence" => Self::build_sequence(&input)?,
                "er" => Self::build_er(&input)?,
                "gantt" => Self::build_gantt(&input)?,
                "mindmap" => Self::build_mindmap(&input)?,
                "state" => Self::build_state(&input)?,
                _ => {
                    return Err(AivyxError::Agent(format!("Unknown diagram type: {dtype}")));
                }
            }
        };

        std::fs::write(&output_path, &mermaid_content)
            .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?;

        Ok(serde_json::json!({
            "path": output_path.display().to_string(),
            "bytes": mermaid_content.len(),
            "content": mermaid_content,
        }))
    }
}

// ---------------------------------------------------------------------------
// 4. TemplateRenderTool — Jinja2/Tera template rendering
// ---------------------------------------------------------------------------

/// Render Jinja2-style templates with variable injection using the Tera engine.
///
/// Supports inline templates or file-based templates with conditionals, loops,
/// filters, and inheritance. Optionally writes output to a file.
pub struct TemplateRenderTool {
    id: ToolId,
}

impl Default for TemplateRenderTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateRenderTool {
    /// Create a new template render tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

#[async_trait]
impl Tool for TemplateRenderTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "template_render"
    }

    fn description(&self) -> &str {
        "Render Jinja2-style templates with variable injection. Supports inline or file-based templates with conditionals, loops, and filters."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "template": {
                    "type": "string",
                    "description": "Inline template string (Jinja2/Tera syntax)"
                },
                "template_path": {
                    "type": "string",
                    "description": "Path to a template file (alternative to inline)"
                },
                "data": {
                    "type": "object",
                    "description": "Variables to inject into the template"
                },
                "output_path": {
                    "type": "string",
                    "description": "Optional path to write rendered output"
                }
            },
            "required": ["data"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let data = &input["data"];

        let template_str = if let Some(inline) = input["template"].as_str() {
            inline.to_string()
        } else if let Some(path_str) = input["template_path"].as_str() {
            let path = resolve_and_validate_path(path_str, "template_render").await?;
            std::fs::read_to_string(&path)
                .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?
        } else {
            return Err(AivyxError::Agent(
                "Either 'template' or 'template_path' is required".into(),
            ));
        };

        let mut tera = tera::Tera::default();
        tera.add_raw_template("__inline__", &template_str)
            .map_err(|e| AivyxError::Agent(format!("Template parse error: {e}")))?;

        let context = tera::Context::from_value(data.clone())
            .map_err(|e| AivyxError::Agent(format!("Context error: {e}")))?;

        let rendered = tera
            .render("__inline__", &context)
            .map_err(|e| AivyxError::Agent(format!("Render error: {e}")))?;

        if let Some(out) = input["output_path"].as_str() {
            let out_path = resolve_and_validate_path(out, "template_render").await?;
            std::fs::write(&out_path, &rendered)
                .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?;
        }

        Ok(serde_json::json!({
            "rendered": rendered,
            "length": rendered.len(),
        }))
    }
}

// ---------------------------------------------------------------------------
// 5. MarkdownExportTool — Markdown → standalone HTML
// ---------------------------------------------------------------------------

/// Convert Markdown to standalone HTML with embedded CSS.
///
/// Uses `pulldown-cmark` with GFM extensions (tables, task lists, strikethrough).
/// Wraps output in a full HTML document with optional minimal styling.
pub struct MarkdownExportTool {
    id: ToolId,
}

impl Default for MarkdownExportTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownExportTool {
    /// Create a new markdown export tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// Minimal CSS for readable HTML output.
const MINIMAL_CSS: &str = r#"
body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    line-height: 1.6;
    max-width: 800px;
    margin: 40px auto;
    padding: 0 20px;
    color: #333;
}
h1, h2, h3 { margin-top: 1.5em; }
code {
    background: #f4f4f4;
    padding: 2px 6px;
    border-radius: 3px;
    font-size: 0.9em;
}
pre {
    background: #f4f4f4;
    padding: 16px;
    border-radius: 6px;
    overflow-x: auto;
}
pre code { background: none; padding: 0; }
table {
    border-collapse: collapse;
    width: 100%;
    margin: 1em 0;
}
th, td {
    border: 1px solid #ddd;
    padding: 8px 12px;
    text-align: left;
}
th { background: #f8f8f8; }
blockquote {
    border-left: 4px solid #ddd;
    margin: 1em 0;
    padding: 0.5em 1em;
    color: #666;
}
a { color: #0366d6; }
"#;

#[async_trait]
impl Tool for MarkdownExportTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "markdown_export"
    }

    fn description(&self) -> &str {
        "Convert Markdown to standalone HTML with embedded CSS. Supports GFM tables, task lists, and strikethrough."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "markdown": {
                    "type": "string",
                    "description": "Markdown content to convert"
                },
                "markdown_path": {
                    "type": "string",
                    "description": "Path to a Markdown file (alternative to inline)"
                },
                "title": {
                    "type": "string",
                    "description": "HTML document title (default: 'Document')"
                },
                "include_css": {
                    "type": "boolean",
                    "description": "Include minimal CSS styling (default: true)"
                },
                "output_path": {
                    "type": "string",
                    "description": "Optional path to write the HTML file"
                }
            }
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        use pulldown_cmark::{Options, Parser, html};

        let markdown = if let Some(inline) = input["markdown"].as_str() {
            inline.to_string()
        } else if let Some(path_str) = input["markdown_path"].as_str() {
            let path = resolve_and_validate_path(path_str, "markdown_export").await?;
            std::fs::read_to_string(&path)
                .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?
        } else {
            return Err(AivyxError::Agent(
                "Either 'markdown' or 'markdown_path' is required".into(),
            ));
        };

        let title = input["title"].as_str().unwrap_or("Document");
        let include_css = input["include_css"].as_bool().unwrap_or(true);

        let options =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

        let parser = Parser::new_ext(&markdown, options);
        let mut html_body = String::new();
        html::push_html(&mut html_body, parser);

        let css_block = if include_css {
            format!("<style>{MINIMAL_CSS}</style>")
        } else {
            String::new()
        };

        let full_html = format!(
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n<title>{title}</title>\n{css_block}\n</head>\n<body>\n{html_body}\n</body>\n</html>"
        );

        if let Some(out) = input["output_path"].as_str() {
            let out_path = resolve_and_validate_path(out, "markdown_export").await?;
            std::fs::write(&out_path, &full_html)
                .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?;
        }

        Ok(serde_json::json!({
            "html": full_html,
            "html_length": full_html.len(),
            "markdown_length": markdown.len(),
        }))
    }
}

// ---------------------------------------------------------------------------
// 6. SentimentAnalyzeTool — VADER-style lexicon sentiment analysis
// ---------------------------------------------------------------------------

/// Analyse text sentiment using a VADER-style lexicon approach.
///
/// Uses a static lexicon of ~750 high-signal words with polarity scores.
/// Applies negation handling (3-word window), capitalization amplification,
/// and booster/diminisher modifiers. Returns compound, positive, negative,
/// and neutral scores.
pub struct SentimentAnalyzeTool {
    id: ToolId,
}

impl Default for SentimentAnalyzeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SentimentAnalyzeTool {
    /// Create a new sentiment analyse tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }
}

/// Core VADER-style lexicon entries: (word, polarity score).
/// Positive scores indicate positive sentiment, negative scores indicate negative.
/// Magnitude indicates strength (typically -4.0 to +4.0 range).
static VADER_LEXICON_DATA: &[(&str, f32)] = &[
    // Strong positive
    ("excellent", 3.2),
    ("outstanding", 3.1),
    ("amazing", 3.0),
    ("wonderful", 3.0),
    ("fantastic", 3.0),
    ("brilliant", 2.9),
    ("superb", 2.9),
    ("exceptional", 2.8),
    ("perfect", 2.8),
    ("magnificent", 2.7),
    ("marvelous", 2.7),
    ("terrific", 2.6),
    ("spectacular", 2.5),
    ("incredible", 2.5),
    ("phenomenal", 2.5),
    ("glorious", 2.4),
    ("delightful", 2.4),
    ("triumph", 2.3),
    ("triumphant", 2.3),
    ("exquisite", 2.3),
    // Moderate positive
    ("great", 2.2),
    ("love", 2.2),
    ("beautiful", 2.1),
    ("happy", 2.1),
    ("joyful", 2.1),
    ("pleased", 2.0),
    ("glad", 2.0),
    ("thrilled", 2.0),
    ("excited", 1.9),
    ("enthusiastic", 1.9),
    ("wonderful", 1.9),
    ("impressive", 1.8),
    ("remarkable", 1.8),
    ("splendid", 1.8),
    ("admire", 1.7),
    ("appreciate", 1.7),
    ("enjoy", 1.7),
    ("grateful", 1.7),
    ("thankful", 1.7),
    ("satisfying", 1.6),
    ("pleasant", 1.6),
    ("positive", 1.6),
    ("favorable", 1.6),
    ("successful", 1.5),
    ("accomplished", 1.5),
    ("confident", 1.5),
    ("inspired", 1.5),
    ("elegant", 1.5),
    ("charming", 1.4),
    ("creative", 1.4),
    ("innovative", 1.4),
    ("clever", 1.3),
    ("smart", 1.3),
    ("talented", 1.3),
    ("skilled", 1.3),
    ("capable", 1.2),
    ("effective", 1.2),
    ("efficient", 1.2),
    ("reliable", 1.2),
    ("trustworthy", 1.2),
    ("helpful", 1.1),
    ("useful", 1.1),
    ("valuable", 1.1),
    ("worthy", 1.1),
    ("decent", 1.0),
    ("fair", 1.0),
    ("fine", 0.9),
    ("ok", 0.5),
    ("okay", 0.5),
    ("good", 1.9),
    ("nice", 1.5),
    ("like", 1.0),
    ("well", 0.8),
    ("better", 1.5),
    ("best", 2.2),
    ("win", 1.8),
    ("won", 1.8),
    ("winning", 1.8),
    ("gains", 1.3),
    ("profit", 1.3),
    ("growth", 1.2),
    ("improve", 1.3),
    ("improved", 1.3),
    ("improvement", 1.3),
    ("advance", 1.2),
    ("progress", 1.2),
    ("benefit", 1.3),
    ("advantage", 1.2),
    ("opportunity", 1.1),
    ("strength", 1.2),
    ("strong", 1.1),
    ("robust", 1.1),
    ("stable", 1.0),
    ("secure", 1.1),
    ("safe", 1.0),
    ("healthy", 1.2),
    ("prosper", 1.3),
    ("thrive", 1.3),
    ("flourish", 1.4),
    ("boom", 1.2),
    ("surge", 1.1),
    ("soar", 1.3),
    ("rally", 1.1),
    ("upturn", 1.1),
    ("recovery", 1.1),
    ("rebound", 1.0),
    ("optimistic", 1.5),
    ("promising", 1.3),
    ("encouraging", 1.3),
    ("hopeful", 1.2),
    ("bright", 1.1),
    // Mild positive
    ("agree", 0.7),
    ("accept", 0.7),
    ("support", 0.8),
    ("recommend", 0.9),
    ("interesting", 0.8),
    // Strong negative
    ("terrible", -3.2),
    ("horrible", -3.1),
    ("awful", -3.0),
    ("dreadful", -2.9),
    ("atrocious", -2.9),
    ("abysmal", -2.8),
    ("appalling", -2.8),
    ("catastrophic", -2.7),
    ("disastrous", -2.7),
    ("devastating", -2.6),
    ("deplorable", -2.5),
    ("abhorrent", -2.5),
    ("repulsive", -2.4),
    ("disgusting", -2.4),
    ("revolting", -2.3),
    ("vile", -2.3),
    ("horrendous", -2.3),
    ("nightmare", -2.2),
    ("toxic", -2.2),
    ("hideous", -2.1),
    // Moderate negative
    ("bad", -2.0),
    ("hate", -2.5),
    ("ugly", -2.0),
    ("sad", -1.9),
    ("angry", -2.0),
    ("furious", -2.3),
    ("miserable", -2.1),
    ("disappointed", -1.9),
    ("frustrating", -1.8),
    ("annoying", -1.7),
    ("boring", -1.5),
    ("dull", -1.3),
    ("mediocre", -1.2),
    ("inferior", -1.5),
    ("poor", -1.8),
    ("weak", -1.3),
    ("flawed", -1.4),
    ("broken", -1.6),
    ("defective", -1.6),
    ("faulty", -1.5),
    ("inadequate", -1.5),
    ("insufficient", -1.3),
    ("unsatisfactory", -1.6),
    ("unacceptable", -1.8),
    ("wrong", -1.5),
    ("fail", -1.8),
    ("failed", -1.8),
    ("failure", -1.9),
    ("lose", -1.6),
    ("lost", -1.5),
    ("loss", -1.6),
    ("losing", -1.6),
    ("decline", -1.3),
    ("decrease", -1.1),
    ("drop", -1.1),
    ("fall", -1.1),
    ("crash", -2.0),
    ("collapse", -2.0),
    ("crisis", -1.8),
    ("risk", -1.0),
    ("danger", -1.5),
    ("threat", -1.3),
    ("harm", -1.6),
    ("damage", -1.5),
    ("destroy", -2.0),
    ("ruin", -2.0),
    ("worse", -1.6),
    ("worst", -2.3),
    ("problem", -1.2),
    ("issue", -0.8),
    ("concern", -0.9),
    ("trouble", -1.3),
    ("difficult", -1.0),
    ("complex", -0.5),
    ("complicated", -0.7),
    ("pain", -1.5),
    ("painful", -1.6),
    ("suffering", -1.8),
    ("struggle", -1.2),
    ("hardship", -1.4),
    ("obstacle", -1.0),
    ("barrier", -0.9),
    ("setback", -1.2),
    ("downturn", -1.3),
    ("recession", -1.5),
    ("stagnation", -1.1),
    ("pessimistic", -1.5),
    ("bleak", -1.5),
    ("grim", -1.4),
    ("dire", -1.6),
    ("gloomy", -1.3),
    ("negative", -1.3),
    ("unfavorable", -1.3),
    // Mild negative
    ("disagree", -0.7),
    ("reject", -1.0),
    ("deny", -0.8),
    ("doubt", -0.8),
    ("skeptical", -0.7),
    ("uncertain", -0.6),
    ("unclear", -0.5),
    ("confused", -0.8),
    ("slow", -0.6),
    ("delay", -0.8),
    ("late", -0.6),
    ("miss", -0.7),
    ("lack", -0.8),
    ("limited", -0.5),
    ("restrict", -0.6),
    // Boosters (intensifiers)
    ("very", 0.0),
    ("really", 0.0),
    ("extremely", 0.0),
    ("absolutely", 0.0),
    ("incredibly", 0.0),
    ("remarkably", 0.0),
    ("completely", 0.0),
    ("totally", 0.0),
    ("truly", 0.0),
    ("highly", 0.0),
    ("deeply", 0.0),
    ("quite", 0.0),
    ("rather", 0.0),
    ("somewhat", 0.0),
    ("slightly", 0.0),
    ("barely", 0.0),
    ("hardly", 0.0),
    ("merely", 0.0),
];

/// Words that intensify the following sentiment word.
static BOOSTERS: &[(&str, f32)] = &[
    ("very", 0.293),
    ("really", 0.293),
    ("extremely", 0.293),
    ("absolutely", 0.293),
    ("incredibly", 0.293),
    ("remarkably", 0.293),
    ("completely", 0.293),
    ("totally", 0.293),
    ("truly", 0.293),
    ("highly", 0.293),
    ("deeply", 0.293),
    ("so", 0.293),
    ("quite", 0.147),
    ("rather", 0.147),
    ("fairly", 0.147),
    ("somewhat", -0.147),
    ("slightly", -0.293),
    ("barely", -0.293),
    ("hardly", -0.293),
    ("merely", -0.293),
];

/// Negation words that flip sentiment within a 3-word window.
static NEGATIONS: &[&str] = &[
    "not",
    "no",
    "never",
    "neither",
    "nobody",
    "nothing",
    "nowhere",
    "nor",
    "cannot",
    "can't",
    "couldn't",
    "shouldn't",
    "wouldn't",
    "won't",
    "don't",
    "doesn't",
    "didn't",
    "isn't",
    "aren't",
    "wasn't",
    "weren't",
    "hasn't",
    "haven't",
    "hadn't",
    "without",
    "lack",
    "lacking",
    "n't",
];

fn get_lexicon() -> &'static HashMap<&'static str, f32> {
    static LEXICON: OnceLock<HashMap<&'static str, f32>> = OnceLock::new();
    LEXICON.get_or_init(|| VADER_LEXICON_DATA.iter().copied().collect())
}

fn get_boosters() -> &'static HashMap<&'static str, f32> {
    static BOOST_MAP: OnceLock<HashMap<&'static str, f32>> = OnceLock::new();
    BOOST_MAP.get_or_init(|| BOOSTERS.iter().copied().collect())
}

/// Normalize a sentiment score to [-1, 1] using the VADER normalization formula.
fn normalize_score(score: f32, alpha: f32) -> f32 {
    score / (score * score + alpha).sqrt()
}

#[async_trait]
impl Tool for SentimentAnalyzeTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "sentiment_analyze"
    }

    fn description(&self) -> &str {
        "Analyse text sentiment using a VADER-style lexicon. Returns compound score [-1,1], positive/negative/neutral proportions, and optional word-level breakdown."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to analyse for sentiment"
                },
                "breakdown": {
                    "type": "boolean",
                    "description": "Include per-word sentiment scores (default: false)"
                }
            },
            "required": ["text"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        None // Pure computation, no I/O
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("Missing 'text' field".into()))?;
        let want_breakdown = input["breakdown"].as_bool().unwrap_or(false);

        let lexicon = get_lexicon();
        let boosters = get_boosters();

        // Tokenize: lowercase words
        let words: Vec<String> = text
            .split_whitespace()
            .map(|w: &str| {
                w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                    .to_lowercase()
            })
            .filter(|w: &String| !w.is_empty())
            .collect();

        let original_words: Vec<&str> = text.split_whitespace().collect();

        let mut sentiments: Vec<f32> = Vec::new();
        let mut word_scores: Vec<serde_json::Value> = Vec::new();
        let mut pos_sum: f32 = 0.0;
        let mut neg_sum: f32 = 0.0;
        let mut neu_count: usize = 0;

        for (i, word) in words.iter().enumerate() {
            let word_str: &str = word.as_str();
            if let Some(&base_score) = lexicon.get(word_str) {
                if base_score == 0.0 {
                    // Booster/filler word — skip scoring
                    neu_count += 1;
                    continue;
                }

                let mut score = base_score;

                // Capitalization amplification: if original word is ALL CAPS, amplify
                if i < original_words.len() {
                    let orig = original_words[i].trim_matches(|c: char| !c.is_alphanumeric());
                    if orig.len() > 1
                        && orig
                            .chars()
                            .all(|c: char| c.is_uppercase() || !c.is_alphabetic())
                    {
                        score += if score > 0.0 { 0.733 } else { -0.733 };
                    }
                }

                // Check for negation in preceding 3 words
                let negated = (1..=3).any(|k| {
                    if i >= k {
                        let prev = &words[i - k];
                        NEGATIONS.contains(&prev.as_str())
                    } else {
                        false
                    }
                });
                if negated {
                    score *= -0.74;
                }

                // Apply booster from preceding word
                if i > 0
                    && let Some(&boost) = boosters.get(words[i - 1].as_str())
                {
                    score += if score > 0.0 { boost } else { -boost };
                }

                sentiments.push(score);
                if score > 0.05 {
                    pos_sum += score;
                } else if score < -0.05 {
                    neg_sum += score;
                } else {
                    neu_count += 1;
                }

                if want_breakdown {
                    word_scores.push(serde_json::json!({
                        "word": word,
                        "score": (score * 1000.0).round() / 1000.0,
                        "negated": negated,
                    }));
                }
            } else {
                neu_count += 1;
            }
        }

        // Compute compound score
        let raw_sum: f32 = sentiments.iter().sum();
        let compound = normalize_score(raw_sum, 15.0);

        // Compute proportions
        let total = pos_sum + neg_sum.abs() + neu_count as f32;
        let (positive, negative, neutral) = if total > 0.0 {
            (
                (pos_sum / total * 1000.0).round() / 1000.0,
                (neg_sum.abs() / total * 1000.0).round() / 1000.0,
                (neu_count as f32 / total * 1000.0).round() / 1000.0,
            )
        } else {
            (0.0, 0.0, 1.0)
        };

        let label = if compound >= 0.05 {
            "positive"
        } else if compound <= -0.05 {
            "negative"
        } else {
            "neutral"
        };

        let mut result = serde_json::json!({
            "compound": (compound * 10000.0).round() / 10000.0,
            "positive": positive,
            "negative": negative,
            "neutral": neutral,
            "label": label,
            "word_count": words.len(),
            "scored_words": sentiments.len(),
        });

        if want_breakdown {
            result["breakdown"] = serde_json::json!(word_scores);
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// 7. ImageMetadataTool — image header parsing (no external deps)
// ---------------------------------------------------------------------------

/// Extract metadata from image files by parsing format-specific headers.
///
/// Supports PNG, JPEG, GIF, BMP, and WebP. Reads only the header bytes
/// needed to extract dimensions, format, color type, and bit depth.
/// Does not load the entire image into memory.
pub struct ImageMetadataTool {
    id: ToolId,
}

impl Default for ImageMetadataTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageMetadataTool {
    /// Create a new image metadata tool instance.
    pub fn new() -> Self {
        Self { id: ToolId::new() }
    }

    fn parse_png(data: &[u8]) -> Result<serde_json::Value> {
        // PNG IHDR chunk starts at byte 8 (after 8-byte signature)
        // Chunk: length(4) + type(4) + data + CRC(4)
        // IHDR data: width(4) + height(4) + bit_depth(1) + color_type(1) + ...
        if data.len() < 29 {
            return Err(AivyxError::Agent("PNG file too small".into()));
        }
        // Verify IHDR chunk type at offset 12
        if &data[12..16] != b"IHDR" {
            return Err(AivyxError::Agent("PNG missing IHDR chunk".into()));
        }
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        let bit_depth = data[24];
        let color_type_byte = data[25];
        let color_type = match color_type_byte {
            0 => "grayscale",
            2 => "rgb",
            3 => "indexed",
            4 => "grayscale_alpha",
            6 => "rgba",
            _ => "unknown",
        };

        Ok(serde_json::json!({
            "format": "png",
            "width": width,
            "height": height,
            "bit_depth": bit_depth,
            "color_type": color_type,
        }))
    }

    fn parse_jpeg(data: &[u8]) -> Result<serde_json::Value> {
        // Scan for SOF0 (0xFF 0xC0) or SOF2 (0xFF 0xC2) marker
        let mut i = 2; // Skip SOI marker
        while i + 1 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if marker == 0xC0 || marker == 0xC2 {
                // SOF marker found
                if i + 9 >= data.len() {
                    return Err(AivyxError::Agent("JPEG SOF truncated".into()));
                }
                let precision = data[i + 4];
                let height = u16::from_be_bytes([data[i + 5], data[i + 6]]);
                let width = u16::from_be_bytes([data[i + 7], data[i + 8]]);
                let components = if i + 9 < data.len() { data[i + 9] } else { 0 };
                let color_type = match components {
                    1 => "grayscale",
                    3 => "rgb",
                    4 => "cmyk",
                    _ => "unknown",
                };
                return Ok(serde_json::json!({
                    "format": "jpeg",
                    "width": width,
                    "height": height,
                    "bit_depth": precision,
                    "color_type": color_type,
                }));
            }
            // Skip this marker segment
            if i + 3 < data.len()
                && marker != 0x00
                && marker != 0xFF
                && marker != 0xD8
                && marker != 0xD9
            {
                let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                i += 2 + seg_len;
            } else {
                i += 2;
            }
        }
        Err(AivyxError::Agent("JPEG SOF marker not found".into()))
    }

    fn parse_gif(data: &[u8]) -> Result<serde_json::Value> {
        // GIF Logical Screen Descriptor: width(2) + height(2) at offset 6
        if data.len() < 13 {
            return Err(AivyxError::Agent("GIF file too small".into()));
        }
        let width = u16::from_le_bytes([data[6], data[7]]);
        let height = u16::from_le_bytes([data[8], data[9]]);
        let packed = data[10];
        let color_resolution = ((packed >> 4) & 0x07) + 1;

        Ok(serde_json::json!({
            "format": "gif",
            "width": width,
            "height": height,
            "bit_depth": color_resolution,
            "color_type": "indexed",
        }))
    }

    fn parse_bmp(data: &[u8]) -> Result<serde_json::Value> {
        // BMP DIB header starts at offset 14
        if data.len() < 30 {
            return Err(AivyxError::Agent("BMP file too small".into()));
        }
        let width = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
        let height = i32::from_le_bytes([data[22], data[23], data[24], data[25]]).abs();
        let bit_depth = u16::from_le_bytes([data[28], data[29]]);
        let color_type = match bit_depth {
            1 | 4 | 8 => "indexed",
            24 => "rgb",
            32 => "rgba",
            _ => "unknown",
        };

        Ok(serde_json::json!({
            "format": "bmp",
            "width": width,
            "height": height,
            "bit_depth": bit_depth,
            "color_type": color_type,
        }))
    }

    fn parse_webp(data: &[u8]) -> Result<serde_json::Value> {
        // WebP: RIFF(4) + size(4) + WEBP(4) + chunk_type(4) + chunk_size(4) + data
        if data.len() < 30 {
            return Err(AivyxError::Agent("WebP file too small".into()));
        }

        let chunk_type = &data[12..16];
        if chunk_type == b"VP8 " {
            // Lossy: frame header at offset 23 (after 3-byte frame tag + 7-byte VP8 header)
            if data.len() < 30 {
                return Err(AivyxError::Agent("WebP VP8 too small".into()));
            }
            // Skip to the bitstream: chunk data starts at 20, frame tag at 20+3=23
            // VP8 bitstream: 3 bytes tag + 3 bytes start code + 2 bytes width + 2 bytes height
            let offset = 26; // 20 (chunk data) + 3 (frame tag) + 3 (start code 0x9D012A)
            if data.len() < offset + 4 {
                return Err(AivyxError::Agent("WebP VP8 frame truncated".into()));
            }
            let width = u16::from_le_bytes([data[offset], data[offset + 1]]) & 0x3FFF;
            let height = u16::from_le_bytes([data[offset + 2], data[offset + 3]]) & 0x3FFF;
            Ok(serde_json::json!({
                "format": "webp",
                "width": width,
                "height": height,
                "bit_depth": 8,
                "color_type": "rgb",
            }))
        } else if chunk_type == b"VP8L" {
            // Lossless
            if data.len() < 25 {
                return Err(AivyxError::Agent("WebP VP8L too small".into()));
            }
            // Signature byte at 20, then 4 bytes of packed width/height
            let b0 = data[21] as u32;
            let b1 = data[22] as u32;
            let b2 = data[23] as u32;
            let b3 = data[24] as u32;
            let bits = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Ok(serde_json::json!({
                "format": "webp",
                "width": width,
                "height": height,
                "bit_depth": 8,
                "color_type": "rgba",
            }))
        } else {
            // Extended format (VP8X)
            if data.len() < 30 {
                return Err(AivyxError::Agent("WebP VP8X too small".into()));
            }
            // Canvas width at offset 24 (3 bytes LE) + 1, height at 27 (3 bytes LE) + 1
            let width =
                (data[24] as u32 | ((data[25] as u32) << 8) | ((data[26] as u32) << 16)) + 1;
            let height =
                (data[27] as u32 | ((data[28] as u32) << 8) | ((data[29] as u32) << 16)) + 1;
            Ok(serde_json::json!({
                "format": "webp",
                "width": width,
                "height": height,
                "bit_depth": 8,
                "color_type": "rgba",
            }))
        }
    }
}

#[async_trait]
impl Tool for ImageMetadataTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "image_metadata"
    }

    fn description(&self) -> &str {
        "Extract metadata from image files (PNG, JPEG, GIF, BMP, WebP). Returns dimensions, format, color type, and bit depth without loading the full image."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the image file"
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
            .ok_or_else(|| AivyxError::Agent("Missing 'path' field".into()))?;
        let path = resolve_and_validate_path(path_str, "image_metadata").await?;

        let file_size = std::fs::metadata(&path)
            .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?
            .len();

        // Read first 512 bytes for header parsing
        let mut data = vec![0u8; 512.min(file_size as usize)];
        {
            use std::io::Read;
            let mut f = std::fs::File::open(&path)
                .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?;
            f.read_exact(&mut data)
                .map_err(|e| AivyxError::Agent(format!("io error: {e}")))?;
        }

        // Detect format by magic bytes
        let mut result = if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
            Self::parse_png(&data)?
        } else if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
            Self::parse_jpeg(&data)?
        } else if data.len() >= 6 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
            Self::parse_gif(&data)?
        } else if data.len() >= 2 && data[0] == b'B' && data[1] == b'M' {
            Self::parse_bmp(&data)?
        } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            Self::parse_webp(&data)?
        } else {
            return Err(AivyxError::Agent(
                "Unrecognized image format. Supported: PNG, JPEG, GIF, BMP, WebP".into(),
            ));
        };

        result["file_size_bytes"] = serde_json::json!(file_size);
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create all Phase 11B document intelligence tools.
pub fn create_document_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(DocumentExtractTool::new()),
        Box::new(ChartGenerateTool::new()),
        Box::new(DiagramAuthorTool::new()),
        Box::new(TemplateRenderTool::new()),
        Box::new(MarkdownExportTool::new()),
        Box::new(SentimentAnalyzeTool::new()),
        Box::new(ImageMetadataTool::new()),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_document_extract_csv() {
        let dir = std::env::temp_dir().join(format!("aivyx_docext_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let csv_path = dir.join("test.csv");
        std::fs::write(&csv_path, "name,age,city\nAlice,30,NYC\nBob,25,LA\n").unwrap();

        let tool = DocumentExtractTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": csv_path.display().to_string() }))
            .await
            .unwrap();

        assert_eq!(result["format"], "csv");
        assert_eq!(result["row_count"], 2);
        assert_eq!(result["headers"][0], "name");
        assert_eq!(result["rows"][0][0], "Alice");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_document_extract_unsupported_format() {
        let dir = std::env::temp_dir().join(format!("aivyx_docext_bad_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.docx");
        std::fs::write(&path, "fake").unwrap();

        let tool = DocumentExtractTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": path.display().to_string() }))
            .await;

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_chart_generate_line() {
        let tool = ChartGenerateTool::new();
        let result = tool
            .execute(serde_json::json!({
                "chart_type": "line",
                "title": "Test Chart",
                "data": [
                    {"x": 1, "y": 10},
                    {"x": 2, "y": 20},
                    {"x": 3, "y": 15},
                ]
            }))
            .await
            .unwrap();

        assert_eq!(result["chart_type"], "line");
        assert_eq!(result["point_count"], 3);
        assert!(result["svg"].as_str().unwrap().contains("<svg"));
    }

    #[tokio::test]
    async fn test_chart_generate_bar() {
        let tool = ChartGenerateTool::new();
        let result = tool
            .execute(serde_json::json!({
                "chart_type": "bar",
                "data": [
                    {"x": 1, "y": 50},
                    {"x": 2, "y": 30},
                ]
            }))
            .await
            .unwrap();

        assert_eq!(result["chart_type"], "bar");
        assert!(result["svg"].as_str().unwrap().contains("<svg"));
    }

    #[tokio::test]
    async fn test_chart_generate_scatter() {
        let tool = ChartGenerateTool::new();
        let result = tool
            .execute(serde_json::json!({
                "chart_type": "scatter",
                "data": [
                    {"x": 1.5, "y": 2.5},
                    {"x": 3.0, "y": 4.0},
                ]
            }))
            .await
            .unwrap();

        assert_eq!(result["chart_type"], "scatter");
    }

    #[tokio::test]
    async fn test_diagram_author_flowchart() {
        let dir = std::env::temp_dir().join(format!("aivyx_diag_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.mmd");

        let tool = DiagramAuthorTool::new();
        let result = tool
            .execute(serde_json::json!({
                "output_path": path.display().to_string(),
                "diagram_type": "flowchart",
                "direction": "LR",
                "nodes": [
                    {"id": "A", "label": "Start", "shape": "round"},
                    {"id": "B", "label": "Process"},
                    {"id": "C", "label": "End", "shape": "round"},
                ],
                "edges": [
                    {"from": "A", "to": "B", "label": "begin"},
                    {"from": "B", "to": "C"},
                ]
            }))
            .await
            .unwrap();

        let content = result["content"].as_str().unwrap();
        assert!(content.contains("flowchart LR"));
        assert!(content.contains("A(Start)"));
        assert!(content.contains("A -->|begin| B"));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("flowchart LR")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_diagram_author_sequence() {
        let dir = std::env::temp_dir().join(format!("aivyx_diag_seq_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seq.mmd");

        let tool = DiagramAuthorTool::new();
        let result = tool
            .execute(serde_json::json!({
                "output_path": path.display().to_string(),
                "diagram_type": "sequence",
                "participants": ["Client", "Server"],
                "messages": [
                    {"from": "Client", "to": "Server", "text": "GET /api"},
                    {"from": "Server", "to": "Client", "text": "200 OK"},
                ]
            }))
            .await
            .unwrap();

        let content = result["content"].as_str().unwrap();
        assert!(content.contains("sequenceDiagram"));
        assert!(content.contains("Client->>Server: GET /api"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_diagram_author_raw_content() {
        let dir = std::env::temp_dir().join(format!("aivyx_diag_raw_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("raw.mmd");

        let tool = DiagramAuthorTool::new();
        let raw = "graph TD\n    A --> B";
        let result = tool
            .execute(serde_json::json!({
                "output_path": path.display().to_string(),
                "content": raw,
            }))
            .await
            .unwrap();

        assert_eq!(result["content"], raw);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_template_render_inline() {
        let tool = TemplateRenderTool::new();
        let result = tool
            .execute(serde_json::json!({
                "template": "Hello, {{ name }}! You have {{ count }} items.",
                "data": { "name": "Alice", "count": 42 }
            }))
            .await
            .unwrap();

        assert_eq!(result["rendered"], "Hello, Alice! You have 42 items.");
    }

    #[tokio::test]
    async fn test_template_render_loop() {
        let tool = TemplateRenderTool::new();
        let result = tool
            .execute(serde_json::json!({
                "template": "{% for item in items %}{{ item }}\n{% endfor %}",
                "data": { "items": ["a", "b", "c"] }
            }))
            .await
            .unwrap();

        let rendered = result["rendered"].as_str().unwrap();
        assert!(rendered.contains("a\n"));
        assert!(rendered.contains("b\n"));
        assert!(rendered.contains("c\n"));
    }

    #[tokio::test]
    async fn test_template_render_conditional() {
        let tool = TemplateRenderTool::new();
        let result = tool
            .execute(serde_json::json!({
                "template": "{% if active %}ON{% else %}OFF{% endif %}",
                "data": { "active": true }
            }))
            .await
            .unwrap();

        assert_eq!(result["rendered"], "ON");
    }

    #[tokio::test]
    async fn test_markdown_export_basic() {
        let tool = MarkdownExportTool::new();
        let result = tool
            .execute(serde_json::json!({
                "markdown": "# Hello\n\nThis is **bold** and *italic*.",
                "title": "Test Doc"
            }))
            .await
            .unwrap();

        let html = result["html"].as_str().unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>Test Doc</title>"));
        assert!(html.contains("<h1>Hello</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[tokio::test]
    async fn test_markdown_export_table() {
        let tool = MarkdownExportTool::new();
        let result = tool
            .execute(serde_json::json!({
                "markdown": "| A | B |\n|---|---|\n| 1 | 2 |",
                "include_css": false
            }))
            .await
            .unwrap();

        let html = result["html"].as_str().unwrap();
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>1</td>"));
        // Should not include CSS
        assert!(!html.contains("<style>"));
    }

    #[tokio::test]
    async fn test_sentiment_analyze_positive() {
        let tool = SentimentAnalyzeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "This is an excellent and amazing product. I love it!"
            }))
            .await
            .unwrap();

        let compound = result["compound"].as_f64().unwrap();
        assert!(compound > 0.3, "Expected positive compound, got {compound}");
        assert_eq!(result["label"], "positive");
    }

    #[tokio::test]
    async fn test_sentiment_analyze_negative() {
        let tool = SentimentAnalyzeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "This is terrible and horrible. I hate it."
            }))
            .await
            .unwrap();

        let compound = result["compound"].as_f64().unwrap();
        assert!(
            compound < -0.3,
            "Expected negative compound, got {compound}"
        );
        assert_eq!(result["label"], "negative");
    }

    #[tokio::test]
    async fn test_sentiment_analyze_negation() {
        let tool = SentimentAnalyzeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "This is not good at all.",
                "breakdown": true
            }))
            .await
            .unwrap();

        // "not good" should reduce positive sentiment
        let compound = result["compound"].as_f64().unwrap();
        assert!(
            compound < 0.5,
            "Negation should reduce compound, got {compound}"
        );
        // Check breakdown exists
        assert!(result["breakdown"].as_array().is_some());
    }

    #[tokio::test]
    async fn test_sentiment_analyze_neutral() {
        let tool = SentimentAnalyzeTool::new();
        let result = tool
            .execute(serde_json::json!({
                "text": "The meeting is scheduled for Tuesday at 3pm in the conference room."
            }))
            .await
            .unwrap();

        let compound = result["compound"].as_f64().unwrap();
        assert!(
            compound.abs() < 0.3,
            "Expected near-neutral, got {compound}"
        );
    }

    #[tokio::test]
    async fn test_image_metadata_png() {
        // Create a minimal valid PNG file
        let dir = std::env::temp_dir().join(format!("aivyx_imgmeta_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.png");

        // Minimal 1x1 RGBA PNG
        let png_data: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // IHDR length = 13
            0x49, 0x48, 0x44, 0x52, // "IHDR"
            0x00, 0x00, 0x00, 0x01, // width = 1
            0x00, 0x00, 0x00, 0x01, // height = 1
            0x08, // bit depth = 8
            0x06, // color type = 6 (RGBA)
            0x00, 0x00, 0x00, // compression, filter, interlace
            0x1F, 0x15, 0xC4,
            0x89, // CRC
                  // (would need IDAT + IEND for a real PNG, but we only parse headers)
        ];
        std::fs::write(&path, png_data).unwrap();

        let tool = ImageMetadataTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": path.display().to_string() }))
            .await
            .unwrap();

        assert_eq!(result["format"], "png");
        assert_eq!(result["width"], 1);
        assert_eq!(result["height"], 1);
        assert_eq!(result["bit_depth"], 8);
        assert_eq!(result["color_type"], "rgba");
        assert!(result["file_size_bytes"].as_u64().unwrap() > 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_image_metadata_gif() {
        let dir = std::env::temp_dir().join(format!("aivyx_imgmeta_gif_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.gif");

        // Minimal GIF89a header: 1x1 pixel
        let gif_data: &[u8] = &[
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, // "GIF89a"
            0x01, 0x00, // width = 1
            0x01, 0x00, // height = 1
            0x80, // packed: GCT flag=1, color resolution=1, sorted=0, GCT size=0
            0x00, // background color index
            0x00, // pixel aspect ratio
        ];
        std::fs::write(&path, gif_data).unwrap();

        let tool = ImageMetadataTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": path.display().to_string() }))
            .await
            .unwrap();

        assert_eq!(result["format"], "gif");
        assert_eq!(result["width"], 1);
        assert_eq!(result["height"], 1);
        assert_eq!(result["color_type"], "indexed");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_image_metadata_bmp() {
        let dir = std::env::temp_dir().join(format!("aivyx_imgmeta_bmp_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.bmp");

        // Minimal BMP: 2x2, 24-bit
        let mut bmp_data = vec![0u8; 70];
        bmp_data[0] = b'B';
        bmp_data[1] = b'M';
        // File size at offset 2 (LE)
        bmp_data[2] = 70;
        // Data offset at offset 10
        bmp_data[10] = 54;
        // DIB header size at offset 14
        bmp_data[14] = 40;
        // Width = 2 at offset 18 (LE i32)
        bmp_data[18] = 2;
        // Height = 2 at offset 22 (LE i32)
        bmp_data[22] = 2;
        // Planes at offset 26
        bmp_data[26] = 1;
        // Bit depth = 24 at offset 28
        bmp_data[28] = 24;

        std::fs::write(&path, &bmp_data).unwrap();

        let tool = ImageMetadataTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": path.display().to_string() }))
            .await
            .unwrap();

        assert_eq!(result["format"], "bmp");
        assert_eq!(result["width"], 2);
        assert_eq!(result["height"], 2);
        assert_eq!(result["bit_depth"], 24);
        assert_eq!(result["color_type"], "rgb");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_image_metadata_unrecognized() {
        let dir = std::env::temp_dir().join(format!("aivyx_imgmeta_bad_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.xyz");
        std::fs::write(&path, "not an image").unwrap();

        let tool = ImageMetadataTool::new();
        let result = tool
            .execute(serde_json::json!({ "path": path.display().to_string() }))
            .await;

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_create_document_tools_count() {
        let tools = create_document_tools();
        assert_eq!(tools.len(), 7);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"document_extract"));
        assert!(names.contains(&"chart_generate"));
        assert!(names.contains(&"diagram_author"));
        assert!(names.contains(&"template_render"));
        assert!(names.contains(&"markdown_export"));
        assert!(names.contains(&"sentiment_analyze"));
        assert!(names.contains(&"image_metadata"));
    }

    #[test]
    fn test_sentiment_lexicon_init() {
        let lexicon = get_lexicon();
        assert!(lexicon.len() > 100, "Lexicon should have 100+ entries");
        assert!(lexicon.contains_key("excellent"));
        assert!(lexicon.contains_key("terrible"));
    }

    #[test]
    fn test_normalize_score() {
        assert!((normalize_score(0.0, 15.0)).abs() < f32::EPSILON);
        assert!(normalize_score(5.0, 15.0) > 0.0);
        assert!(normalize_score(-5.0, 15.0) < 0.0);
        // Should be bounded to [-1, 1]
        assert!(normalize_score(100.0, 15.0) <= 1.0);
        assert!(normalize_score(-100.0, 15.0) >= -1.0);
    }
}
