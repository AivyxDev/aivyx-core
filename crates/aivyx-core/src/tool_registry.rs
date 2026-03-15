use std::collections::HashMap;

use crate::id::ToolId;
use crate::traits::Tool;

/// Registry of available tools, keyed by [`ToolId`].
///
/// The agent runtime uses this to look up tools by ID or name, and to
/// generate tool definitions for the LLM's `tools` parameter.
///
/// Maintains a secondary name→ID index for O(1) lookup by name.
pub struct ToolRegistry {
    tools: HashMap<ToolId, Box<dyn Tool>>,
    name_index: HashMap<String, ToolId>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            name_index: HashMap::new(),
        }
    }

    /// Register a tool. Replaces any existing tool with the same ID.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let id = tool.id();
        let name = tool.name().to_string();
        // Remove old name index entry if replacing an existing tool ID
        if let Some(old_tool) = self.tools.get(&id) {
            self.name_index.remove(old_tool.name());
        }
        self.name_index.insert(name, id);
        self.tools.insert(id, tool);
    }

    /// Look up a tool by its ID.
    pub fn get(&self, id: &ToolId) -> Option<&dyn Tool> {
        self.tools.get(id).map(|t| t.as_ref())
    }

    /// Look up a tool by its human-readable name (O(1) via index).
    pub fn get_by_name(&self, name: &str) -> Option<&dyn Tool> {
        self.name_index
            .get(name)
            .and_then(|id| self.tools.get(id))
            .map(|t| t.as_ref())
    }

    /// List all registered tools.
    pub fn list(&self) -> Vec<&dyn Tool> {
        self.tools.values().map(|t| t.as_ref()).collect()
    }

    /// Remove a tool by its ID. Returns the removed tool, if any.
    pub fn unregister(&mut self, id: &ToolId) -> Option<Box<dyn Tool>> {
        if let Some(tool) = self.tools.remove(id) {
            self.name_index.remove(tool.name());
            Some(tool)
        } else {
            None
        }
    }

    /// Remove a tool by its human-readable name. Returns the removed tool, if any.
    pub fn unregister_by_name(&mut self, name: &str) -> Option<Box<dyn Tool>> {
        if let Some(id) = self.name_index.remove(name) {
            self.tools.remove(&id)
        } else {
            None
        }
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns true if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Check if a tool with this name is already registered.
    pub fn has_name(&self, name: &str) -> bool {
        self.name_index.contains_key(name)
    }

    /// Generate tool definitions suitable for the LLM's `tools` parameter.
    ///
    /// Returns a JSON array of objects with `name`, `description`, and
    /// `input_schema` fields.
    pub fn tool_definitions(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "input_schema": tool.input_schema(),
                })
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use async_trait::async_trait;

    struct DummyTool {
        tool_id: ToolId,
        tool_name: String,
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn id(&self) -> ToolId {
            self.tool_id
        }
        fn name(&self) -> &str {
            &self.tool_name
        }
        fn description(&self) -> &str {
            "A dummy tool for testing"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
            Ok(serde_json::json!({"result": "ok"}))
        }
    }

    #[test]
    fn register_and_get() {
        let mut reg = ToolRegistry::new();
        let id = ToolId::new();
        reg.register(Box::new(DummyTool {
            tool_id: id,
            tool_name: "test_tool".into(),
        }));

        assert!(reg.get(&id).is_some());
        assert_eq!(reg.get(&id).unwrap().name(), "test_tool");
    }

    #[test]
    fn get_by_name() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "finder".into(),
        }));

        assert!(reg.get_by_name("finder").is_some());
        assert!(reg.get_by_name("nonexistent").is_none());
    }

    #[test]
    fn list_tools() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "alpha".into(),
        }));
        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "beta".into(),
        }));

        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn unregister_by_id() {
        let mut reg = ToolRegistry::new();
        let id = ToolId::new();
        reg.register(Box::new(DummyTool {
            tool_id: id,
            tool_name: "removable".into(),
        }));
        assert_eq!(reg.len(), 1);

        let removed = reg.unregister(&id);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name(), "removable");
        assert!(reg.is_empty());
    }

    #[test]
    fn unregister_by_name_found() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "target".into(),
        }));
        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "keeper".into(),
        }));

        let removed = reg.unregister_by_name("target");
        assert!(removed.is_some());
        assert_eq!(reg.len(), 1);
        assert!(reg.get_by_name("keeper").is_some());
    }

    #[test]
    fn unregister_by_name_not_found() {
        let mut reg = ToolRegistry::new();
        assert!(reg.unregister_by_name("ghost").is_none());
    }

    #[test]
    fn len_and_is_empty() {
        let mut reg = ToolRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);

        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "one".into(),
        }));
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn name_index_lookup() {
        let mut reg = ToolRegistry::new();
        let id = ToolId::new();
        reg.register(Box::new(DummyTool {
            tool_id: id,
            tool_name: "indexed_tool".into(),
        }));

        // Name index should map directly to the tool
        let tool = reg.get_by_name("indexed_tool").unwrap();
        assert_eq!(tool.id(), id);

        // Verify the internal index is maintained
        assert_eq!(reg.name_index.len(), 1);
        assert_eq!(reg.name_index.get("indexed_tool"), Some(&id));
    }

    #[test]
    fn name_index_after_unregister() {
        let mut reg = ToolRegistry::new();
        let id = ToolId::new();
        reg.register(Box::new(DummyTool {
            tool_id: id,
            tool_name: "removable".into(),
        }));
        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "keeper".into(),
        }));
        assert_eq!(reg.name_index.len(), 2);

        // Unregister by ID
        reg.unregister(&id);
        assert!(reg.get_by_name("removable").is_none());
        assert_eq!(reg.name_index.len(), 1);

        // Unregister by name
        reg.unregister_by_name("keeper");
        assert!(reg.get_by_name("keeper").is_none());
        assert!(reg.name_index.is_empty());
    }

    #[test]
    fn has_name_checks_index() {
        let mut reg = ToolRegistry::new();
        assert!(!reg.has_name("echo"));

        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "echo".into(),
        }));
        assert!(reg.has_name("echo"));
        assert!(!reg.has_name("other"));
    }

    #[test]
    fn tool_definitions_format() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(DummyTool {
            tool_id: ToolId::new(),
            tool_name: "my_tool".into(),
        }));

        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["name"], "my_tool");
        assert!(defs[0]["input_schema"].is_object());
    }
}
