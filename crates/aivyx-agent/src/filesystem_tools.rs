//! Filesystem tools: read, write, delete, move, copy files and list directories.

use std::path::PathBuf;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::built_in_tools::resolve_and_validate_path;

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

/// Create all filesystem tools.
pub fn create_filesystem_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(FileReadTool::new()),
        Box::new(FileWriteTool::new()),
        Box::new(FileDeleteTool::new()),
        Box::new(FileMoveTool::new()),
        Box::new(FileCopyTool::new()),
        Box::new(DirectoryListTool::new()),
    ]
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

        let _ = std::fs::remove_dir_all(&root);
    }

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
}
