use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::mpsc;
use tracing::debug;

use aivyx_core::{AivyxError, Result};

use crate::message::{
    ChatMessage, ChatRequest, ChatResponse, Content, ContentBlock, ImageSource, Role, StopReason,
    TokenUsage, ToolCall,
};
use crate::provider::{LlmProvider, StreamEvent};

/// Claude Messages API provider.
pub struct ClaudeProvider {
    client: Client,
    api_key: SecretString,
    model: String,
    api_url: String,
}

impl ClaudeProvider {
    pub fn new(api_key: SecretString, model: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10))
                .pool_max_idle_per_host(4)
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .expect("failed to build HTTP client"),
            api_key,
            model,
            api_url: "https://api.anthropic.com/v1/messages".into(),
        }
    }

    /// Build the request body for the Claude Messages API.
    fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        let model = request.model.as_deref().unwrap_or(&self.model);

        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .filter_map(|msg| self.map_message(msg))
            .collect();

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": request.max_tokens,
            "messages": messages,
        });

        if let Some(system) = &request.system_prompt {
            body["system"] = serde_json::json!(system);
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::json!(request.tools);
        }

        body
    }

    fn map_message(&self, msg: &ChatMessage) -> Option<serde_json::Value> {
        match msg.role {
            Role::System => None, // System is handled separately in Claude API
            Role::User => {
                let api_content = match &msg.content {
                    Content::Text(s) => serde_json::json!(s),
                    Content::Blocks(blocks) => {
                        let mapped: Vec<serde_json::Value> = blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } => {
                                    Some(serde_json::json!({"type": "text", "text": text}))
                                }
                                ContentBlock::Image { source } => Some(match source {
                                    ImageSource::Base64 { media_type, data } => {
                                        serde_json::json!({
                                            "type": "image",
                                            "source": {
                                                "type": "base64",
                                                "media_type": media_type,
                                                "data": data,
                                            }
                                        })
                                    }
                                    ImageSource::Url { url } => {
                                        serde_json::json!({
                                            "type": "image",
                                            "source": {
                                                "type": "url",
                                                "url": url,
                                            }
                                        })
                                    }
                                }),
                            })
                            .collect();
                        serde_json::json!(mapped)
                    }
                };
                Some(serde_json::json!({"role": "user", "content": api_content}))
            }
            Role::Assistant => {
                let mut content: Vec<serde_json::Value> = Vec::new();
                let text = msg.content.text();
                if !text.is_empty() {
                    content.push(serde_json::json!({
                        "type": "text",
                        "text": text,
                    }));
                }
                for tc in &msg.tool_calls {
                    content.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": tc.arguments,
                    }));
                }
                Some(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }))
            }
            Role::Tool => msg.tool_result.as_ref().map(|result| {
                serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": result.tool_call_id,
                        "content": serde_json::to_string(&result.content).unwrap_or_default(),
                        "is_error": result.is_error,
                    }],
                })
            }),
        }
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<ChatResponse> {
        let stop_reason = match body["stop_reason"].as_str().unwrap_or("end_turn") {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            other => {
                return Err(AivyxError::LlmProvider(format!(
                    "unknown stop_reason: {other}"
                )));
            }
        };

        let mut text = String::new();
        let mut tool_calls = Vec::new();

        if let Some(content) = body["content"].as_array() {
            for block in content {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            text.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        tool_calls.push(ToolCall {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                            arguments: block["input"].clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        let usage = TokenUsage {
            input_tokens: body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
        };

        let message = if tool_calls.is_empty() {
            ChatMessage::assistant(text)
        } else {
            ChatMessage::assistant_with_tool_calls(text, tool_calls)
        };

        Ok(ChatResponse {
            message,
            usage,
            stop_reason,
        })
    }
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    fn name(&self) -> &str {
        "claude"
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let body = self.build_request_body(request);

        debug!("Sending Claude API request");

        let response = self
            .client
            .post(&self.api_url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("Claude API request failed: {e}")))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AivyxError::RateLimit(
                "Claude API rate limit exceeded".into(),
            ));
        }

        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::LlmProvider(format!(
                "Claude API error {status}: {error_body}"
            )));
        }

        let response_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AivyxError::LlmProvider(format!("failed to parse response: {e}")))?;

        self.parse_response(&response_body)
    }

    async fn chat_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        use futures_util::StreamExt;

        let mut body = self.build_request_body(request);
        body["stream"] = serde_json::json!(true);

        debug!("Sending Claude API streaming request");

        let response = self
            .client
            .post(&self.api_url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("Claude API request failed: {e}")))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AivyxError::RateLimit(
                "Claude API rate limit exceeded".into(),
            ));
        }
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::LlmProvider(format!(
                "Claude API error {status}: {error_body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut full_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage = TokenUsage::default();
        let mut stop_reason = StopReason::EndTurn;
        // Track tool_use blocks being assembled across deltas
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_input = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AivyxError::Http(format!("stream read error: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end().to_string();
                buffer = buffer[line_end + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        let event_type = event["type"].as_str().unwrap_or("");
                        match event_type {
                            "content_block_start" => {
                                let block = &event["content_block"];
                                if block["type"].as_str() == Some("tool_use") {
                                    current_tool_id =
                                        block["id"].as_str().unwrap_or("").to_string();
                                    current_tool_name =
                                        block["name"].as_str().unwrap_or("").to_string();
                                    current_tool_input.clear();
                                }
                            }
                            "content_block_delta" => {
                                let delta = &event["delta"];
                                match delta["type"].as_str() {
                                    Some("text_delta") => {
                                        if let Some(text) = delta["text"].as_str() {
                                            full_text.push_str(text);
                                            let _ = tx
                                                .send(StreamEvent::TextDelta(text.to_string()))
                                                .await;
                                        }
                                    }
                                    Some("input_json_delta") => {
                                        if let Some(json) = delta["partial_json"].as_str() {
                                            current_tool_input.push_str(json);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "content_block_stop" => {
                                if !current_tool_name.is_empty() {
                                    let arguments: serde_json::Value =
                                        serde_json::from_str(&current_tool_input)
                                            .unwrap_or(serde_json::json!({}));
                                    tool_calls.push(ToolCall {
                                        id: std::mem::take(&mut current_tool_id),
                                        name: std::mem::take(&mut current_tool_name),
                                        arguments,
                                    });
                                    current_tool_input.clear();
                                }
                            }
                            "message_delta" => {
                                let delta = &event["delta"];
                                if let Some(sr) = delta["stop_reason"].as_str() {
                                    stop_reason = match sr {
                                        "end_turn" => StopReason::EndTurn,
                                        "max_tokens" => StopReason::MaxTokens,
                                        "tool_use" => StopReason::ToolUse,
                                        _ => StopReason::EndTurn,
                                    };
                                }
                                if let Some(u) = event.get("usage")
                                    && let Some(out) = u["output_tokens"].as_u64()
                                {
                                    usage.output_tokens = out as u32;
                                }
                            }
                            "message_start" => {
                                if let Some(u) = event["message"].get("usage") {
                                    usage.input_tokens =
                                        u["input_tokens"].as_u64().unwrap_or(0) as u32;
                                }
                            }
                            "message_stop" => {
                                // Stream complete
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let message = if tool_calls.is_empty() {
            ChatMessage::assistant(full_text)
        } else {
            ChatMessage::assistant_with_tool_calls(full_text, tool_calls)
        };

        let _ = tx
            .send(StreamEvent::Done {
                usage,
                stop_reason,
                message,
            })
            .await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_basic() {
        let provider = ClaudeProvider::new(
            SecretString::from("test-key".to_string()),
            "claude-sonnet-4-20250514".into(),
        );

        let request = ChatRequest {
            system_prompt: Some("You are helpful.".into()),
            messages: vec![ChatMessage::user("Hello")],
            tools: vec![],
            model: None,
            max_tokens: 1024,
        };

        let body = provider.build_request_body(&request);
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["system"], "You are helpful.");
    }

    #[test]
    fn parse_response_text() {
        let provider = ClaudeProvider::new(SecretString::from("key".to_string()), "model".into());

        let body = serde_json::json!({
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5},
        });

        let resp = provider.parse_response(&body).unwrap();
        assert_eq!(resp.message.content.text(), "Hello!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 10);
    }

    #[test]
    fn parse_response_tool_use() {
        let provider = ClaudeProvider::new(SecretString::from("key".to_string()), "model".into());

        let body = serde_json::json!({
            "content": [
                {"type": "text", "text": "I'll search for that."},
                {"type": "tool_use", "id": "tc_1", "name": "web_search", "input": {"query": "rust"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 15},
        });

        let resp = provider.parse_response(&body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].name, "web_search");
    }
}
