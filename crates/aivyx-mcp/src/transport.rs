//! MCP transport implementations for stdio and HTTP+SSE.
//!
//! The transport layer handles the low-level communication with MCP servers.
//! Two transports are supported:
//! - **Stdio**: Spawns a child process and communicates via stdin/stdout
//! - **SSE (HTTP)**: Posts JSON-RPC requests to an HTTP endpoint

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use aivyx_core::{AivyxError, Result};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Trait for MCP transport layers that send requests and receive responses.
#[async_trait]
pub trait McpTransportLayer: Send + Sync {
    /// Send a JSON-RPC request and wait for the matching response.
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(&self, request: &JsonRpcRequest) -> Result<()>;

    /// Gracefully shut down the transport.
    async fn shutdown(&self) -> Result<()>;
}

/// Pending response tracker — maps request ID to oneshot sender.
type PendingMap = HashMap<u64, oneshot::Sender<JsonRpcResponse>>;

/// Stdio transport: communicates with an MCP server via child process stdin/stdout.
///
/// Spawns the MCP server as a child process. JSON-RPC messages are written
/// to stdin (newline-delimited) and responses are read from stdout by a
/// background reader task.
pub struct StdioTransport {
    /// Handle to child process stdin for writing requests.
    stdin: Mutex<tokio::process::ChildStdin>,
    /// Pending response waiters, shared with the background reader.
    pending: Arc<Mutex<PendingMap>>,
    /// Background reader task handle.
    _reader_handle: tokio::task::JoinHandle<()>,
    /// Channel to signal shutdown to the reader task.
    shutdown_tx: mpsc::Sender<()>,
    /// Child process handle for cleanup.
    child: Mutex<Option<Child>>,
    /// Timeout for individual requests.
    timeout: Duration,
}

impl StdioTransport {
    /// Spawn a child process and set up stdio communication.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        timeout: Duration,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AivyxError::Other(format!("failed to spawn MCP server '{command}': {e}"))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AivyxError::Other("MCP server stdin not available".into()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AivyxError::Other("MCP server stdout not available".into()))?;

        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(HashMap::new()));
        let pending_reader = Arc::clone(&pending);

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // Background task: read JSON-RPC responses from stdout line by line.
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();

            loop {
                line.clear();
                tokio::select! {
                    result = reader.read_line(&mut line) => {
                        match result {
                            Ok(0) => break, // EOF — child exited
                            Ok(_) => {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                                    Ok(resp) => {
                                        if let Some(id) = resp.id {
                                            let mut map = pending_reader.lock().await;
                                            if let Some(sender) = map.remove(&id) {
                                                let _ = sender.send(resp);
                                            }
                                        }
                                        // Notifications (no id) are silently ignored
                                    }
                                    Err(e) => {
                                        tracing::debug!(
                                            "MCP stdout parse error: {e} — line: {trimmed}"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("MCP stdout read error: {e}");
                                break;
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            stdin: Mutex::new(stdin),
            pending,
            _reader_handle: reader_handle,
            shutdown_tx,
            child: Mutex::new(Some(child)),
            timeout,
        })
    }

    /// Write a JSON-RPC message to the child's stdin.
    async fn write_message(&self, request: &JsonRpcRequest) -> Result<()> {
        let mut json = serde_json::to_string(request).map_err(AivyxError::Serialization)?;
        json.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|e| AivyxError::Other(format!("MCP stdin write error: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| AivyxError::Other(format!("MCP stdin flush error: {e}")))?;

        Ok(())
    }
}

#[async_trait]
impl McpTransportLayer for StdioTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let id = request.id.ok_or_else(|| {
            AivyxError::Other("cannot send() a notification — use notify()".into())
        })?;

        // Register a pending response waiter.
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        // Write the request.
        if let Err(e) = self.write_message(request).await {
            // Clean up the pending entry on write failure.
            let mut map = self.pending.lock().await;
            map.remove(&id);
            return Err(e);
        }

        // Wait for the response with a configurable timeout.
        tokio::time::timeout(self.timeout, rx)
            .await
            .map_err(|_| {
                AivyxError::Other(format!(
                    "MCP request {id} timed out after {:?}",
                    self.timeout
                ))
            })?
            .map_err(|_| AivyxError::Other("MCP response channel closed".into()))
    }

    async fn notify(&self, request: &JsonRpcRequest) -> Result<()> {
        self.write_message(request).await
    }

    async fn shutdown(&self) -> Result<()> {
        let _ = self.shutdown_tx.send(()).await;
        if let Some(mut child) = self.child.lock().await.take() {
            // Give the child a moment to exit gracefully, then kill.
            tokio::select! {
                _ = child.wait() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    let _ = child.kill().await;
                }
            }
        }
        Ok(())
    }
}

/// HTTP+SSE transport: communicates with an MCP server via HTTP POST requests.
///
/// For the streamable HTTP transport, JSON-RPC requests are sent as POST
/// requests and responses come back in the HTTP response body.
pub struct SseTransport {
    /// HTTP client instance.
    client: reqwest::Client,
    /// Server endpoint URL.
    url: String,
}

impl SseTransport {
    /// Create a new SSE transport targeting the given URL with a configurable timeout.
    pub fn new(url: &str, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();

        Self {
            client,
            url: url.to_string(),
        }
    }
}

#[async_trait]
impl McpTransportLayer for SseTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let resp = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|e| AivyxError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(AivyxError::Http(format!(
                "MCP server returned HTTP {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| AivyxError::Http(format!("MCP response body read error: {e}")))?;

        serde_json::from_str(&body)
            .map_err(|e| AivyxError::Other(format!("MCP response parse error: {e} — body: {body}")))
    }

    async fn notify(&self, request: &JsonRpcRequest) -> Result<()> {
        let resp = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|e| AivyxError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(AivyxError::Http(format!(
                "MCP server returned HTTP {}",
                resp.status()
            )));
        }

        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        // HTTP transport has no persistent connection to close.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_transport_creation() {
        let transport = SseTransport::new("http://localhost:3001/mcp", Duration::from_secs(60));
        assert_eq!(transport.url, "http://localhost:3001/mcp");
    }

    #[test]
    fn sse_transport_custom_timeout() {
        let transport = SseTransport::new("http://localhost:3001/mcp", Duration::from_secs(120));
        assert_eq!(transport.url, "http://localhost:3001/mcp");
    }
}
