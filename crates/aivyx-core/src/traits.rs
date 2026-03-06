use async_trait::async_trait;

use crate::error::Result;
use crate::id::ToolId;
use crate::scope::CapabilityScope;

/// A tool that an agent can invoke.
///
/// Uses `#[async_trait]` for object safety (`Box<dyn Tool>`).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the unique identifier for this tool.
    fn id(&self) -> ToolId;
    /// Returns the human-readable name of this tool.
    fn name(&self) -> &str;
    /// Returns a brief description of what this tool does.
    fn description(&self) -> &str;
    /// Returns the JSON Schema describing the tool's expected input.
    fn input_schema(&self) -> serde_json::Value;

    /// Returns the capability scope required to execute this tool, or `None`
    /// if the tool requires no capability check (always allowed).
    fn required_scope(&self) -> Option<CapabilityScope> {
        None
    }

    /// Execute the tool with the given JSON input, returning JSON output.
    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value>;
}

/// Adapter for communication channels (e.g., CLI, HTTP, WebSocket).
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Send a text message through the channel.
    async fn send(&self, message: &str) -> Result<()>;

    /// Receive the next text message from the channel.
    async fn receive(&self) -> Result<String>;
}
