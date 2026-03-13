//! Export audit log entries to JSON or CSV format.

use std::io::Write;

use aivyx_core::Result;

use crate::log::AuditLog;

/// Export the full audit log as a JSON array.
///
/// Writes a JSON array of all [`AuditEntry`](crate::AuditEntry) objects
/// to the provided writer.
pub fn export_json(log: &AuditLog, writer: &mut dyn Write) -> Result<()> {
    let entries = log.read_all_entries()?;
    let json = serde_json::to_string_pretty(&entries)?;
    writer
        .write_all(json.as_bytes())
        .map_err(aivyx_core::AivyxError::Io)?;
    Ok(())
}

/// Export the audit log as CSV.
///
/// Writes a header line followed by one row per entry:
/// `sequence_number,timestamp,event_type,hmac`
pub fn export_csv(log: &AuditLog, writer: &mut dyn Write) -> Result<()> {
    let entries = log.read_all_entries()?;

    writeln!(writer, "sequence_number,timestamp,event_type,hmac")
        .map_err(aivyx_core::AivyxError::Io)?;

    for entry in &entries {
        // Extract the event type from the serde tag
        let event_type = extract_event_type(&entry.event);
        writeln!(
            writer,
            "{},{},{},{}",
            entry.sequence_number,
            csv_escape(&entry.timestamp),
            csv_escape(&event_type),
            &entry.hmac
        )
        .map_err(aivyx_core::AivyxError::Io)?;
    }

    Ok(())
}

/// Extract the serde tag ("type" field) from an AuditEvent.
fn extract_event_type(event: &crate::event::AuditEvent) -> String {
    // Serialize to JSON and extract the "type" field
    if let Ok(json) = serde_json::to_value(event)
        && let Some(t) = json.get("type").and_then(|v| v.as_str())
    {
        return t.to_string();
    }
    "Unknown".to_string()
}

/// Escape a CSV field value.
///
/// Wraps in quotes when the value contains commas, quotes, or newlines.
/// Prefixes with a single quote when the value starts with `=`, `+`, `-`,
/// or `@` to neutralize formula injection in spreadsheet applications.
fn csv_escape(s: &str) -> String {
    let s = if s.starts_with('=') || s.starts_with('+') || s.starts_with('-') || s.starts_with('@')
    {
        // Tab prefix is the OWASP-recommended mitigation for CSV injection.
        // A leading tab is stripped by most spreadsheet apps but prevents
        // the cell from being interpreted as a formula.
        format!("\t{s}")
    } else {
        s.to_string()
    };

    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\t') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AuditEvent;
    use chrono::Utc;

    fn test_log() -> (AuditLog, std::path::PathBuf) {
        let name = format!("aivyx_export_test_{}.jsonl", uuid::Uuid::new_v4());
        let path = std::env::temp_dir().join(name);
        let log = AuditLog::new(&path, b"export-test-key!!!!!!!!!!!!!!!!!");
        log.append(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        })
        .unwrap();
        log.append(AuditEvent::AuditVerified {
            entries_checked: 1,
            valid: true,
        })
        .unwrap();
        (log, path)
    }

    #[test]
    fn export_json_valid() {
        let (log, path) = test_log();
        let mut buf = Vec::new();
        export_json(&log, &mut buf).unwrap();

        let json: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["sequence_number"], 0);
        assert_eq!(json[1]["sequence_number"], 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn export_csv_valid() {
        let (log, path) = test_log();
        let mut buf = Vec::new();
        export_csv(&log, &mut buf).unwrap();

        let csv = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "sequence_number,timestamp,event_type,hmac");
        assert!(lines[1].starts_with("0,"));
        assert!(lines[1].contains("SystemInit"));
        assert!(lines[2].starts_with("1,"));
        assert!(lines[2].contains("AuditVerified"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn csv_escape_neutralizes_formula_injection() {
        // Cells starting with formula triggers must be prefixed
        let escaped = csv_escape("=cmd|'/C calc'!A0");
        assert!(
            escaped.starts_with("\"\t="),
            "expected tab-prefix, got: {escaped}"
        );
        assert!(!escaped.starts_with("\"="));

        let escaped = csv_escape("+1+1");
        assert!(escaped.contains("\t+"));

        let escaped = csv_escape("-1-1");
        assert!(escaped.contains("\t-"));

        let escaped = csv_escape("@SUM(A1:A10)");
        assert!(escaped.contains("\t@"));

        // Normal text should pass through unchanged
        assert_eq!(csv_escape("hello"), "hello");
    }
}
