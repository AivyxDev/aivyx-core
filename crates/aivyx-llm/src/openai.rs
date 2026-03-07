use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::mpsc;
use tracing::debug;

use aivyx_core::{AivyxError, Result};

use crate::message::{
    ChatMessage, ChatRequest, ChatResponse, Role, StopReason, TokenUsage, ToolCall,
};
use crate::provider::{LlmProvider, StreamEvent};

/// OpenAI Chat Completions API provider.
pub struct OpenAIProvider {
    client: Client,
    api_key: SecretString,
    model: String,
    api_url: String,
}

impl OpenAIProvider {
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
            api_url: "https://api.openai.com/v1/chat/completions".into(),
        }
    }

    fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        let model = request.model.as_deref().unwrap_or(&self.model);

        let mut messages: Vec<serde_json::Value> = Vec::new();

        if let Some(system) = &request.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": system,
            }));
        }

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    messages.push(serde_json::json!({
                        "role": "system",
                        "content": msg.content,
                    }));
                }
                Role::User => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content,
                    }));
                }
                Role::Assistant => {
                    let mut entry = serde_json::json!({
                        "role": "assistant",
                        "content": msg.content,
                    });
                    if !msg.tool_calls.is_empty() {
                        let tcs: Vec<serde_json::Value> = msg
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                                    }
                                })
                            })
                            .collect();
                        entry["tool_calls"] = serde_json::json!(tcs);
                    }
                    messages.push(entry);
                }
                Role::Tool => {
                    if let Some(result) = &msg.tool_result {
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": result.tool_call_id,
                            "content": serde_json::to_string(&result.content).unwrap_or_default(),
                        }));
                    }
                }
            }
        }

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens,
        });

        if !request.tools.is_empty() {
            let functions: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t["name"],
                            "description": t["description"],
                            "parameters": t["input_schema"],
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(functions);
        }

        body
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<ChatResponse> {
        let choice = body["choices"]
            .get(0)
            .ok_or_else(|| AivyxError::LlmProvider("no choices in response".into()))?;

        let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
        let stop_reason = match finish_reason {
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            "tool_calls" => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };

        let msg = &choice["message"];
        let content = msg["content"].as_str().unwrap_or_default().to_string();

        let mut tool_calls = Vec::new();
        if let Some(tcs) = msg["tool_calls"].as_array() {
            for tc in tcs {
                let function = &tc["function"];
                let args_str = function["arguments"].as_str().unwrap_or("{}");
                let arguments: serde_json::Value =
                    serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                tool_calls.push(ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_string(),
                    name: function["name"].as_str().unwrap_or_default().to_string(),
                    arguments,
                });
            }
        }

        let usage = if let Some(u) = body.get("usage") {
            TokenUsage {
                input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                output_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            }
        } else {
            TokenUsage::default()
        };

        let message = if tool_calls.is_empty() {
            ChatMessage::assistant(content)
        } else {
            ChatMessage::assistant_with_tool_calls(content, tool_calls)
        };

        Ok(ChatResponse {
            message,
            usage,
            stop_reason,
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let body = self.build_request_body(request);

        debug!("Sending OpenAI API request");

        let response = self
            .client
            .post(&self.api_url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("OpenAI API request failed: {e}")))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AivyxError::RateLimit(
                "OpenAI API rate limit exceeded".into(),
            ));
        }

        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::LlmProvider(format!(
                "OpenAI API error {status}: {error_body}"
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

        debug!("Sending OpenAI API streaming request");

        let response = self
            .client
            .post(&self.api_url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("OpenAI API request failed: {e}")))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AivyxError::RateLimit(
                "OpenAI API rate limit exceeded".into(),
            ));
        }
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::LlmProvider(format!(
                "OpenAI API error {status}: {error_body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut full_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage = TokenUsage::default();
        let mut stop_reason = StopReason::EndTurn;
        // Track tool call assembly across deltas
        let mut tc_index_map: std::collections::HashMap<u64, (String, String, String)> =
            std::collections::HashMap::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AivyxError::Http(format!("stream read error: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end().to_string();
                buffer = buffer[line_end + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(choice) = event["choices"].get(0) {
                            let delta = &choice["delta"];

                            // Text content
                            if let Some(content) = delta["content"].as_str() {
                                full_text.push_str(content);
                                let _ = tx.send(StreamEvent::TextDelta(content.to_string())).await;
                            }

                            // Tool calls
                            if let Some(tcs) = delta["tool_calls"].as_array() {
                                for tc in tcs {
                                    let idx = tc["index"].as_u64().unwrap_or(0);
                                    let entry = tc_index_map.entry(idx).or_insert_with(|| {
                                        (String::new(), String::new(), String::new())
                                    });

                                    if let Some(id) = tc["id"].as_str() {
                                        entry.0 = id.to_string();
                                    }
                                    if let Some(name) = tc["function"]["name"].as_str() {
                                        entry.1 = name.to_string();
                                    }
                                    if let Some(args) = tc["function"]["arguments"].as_str() {
                                        entry.2.push_str(args);
                                    }
                                }
                            }

                            // Finish reason
                            if let Some(fr) = choice["finish_reason"].as_str() {
                                stop_reason = match fr {
                                    "stop" => StopReason::EndTurn,
                                    "length" => StopReason::MaxTokens,
                                    "tool_calls" => StopReason::ToolUse,
                                    _ => StopReason::EndTurn,
                                };
                            }
                        }

                        // Usage (OpenAI includes usage in the final chunk when stream_options.include_usage is set,
                        // but we also handle it if present)
                        if let Some(u) = event.get("usage") {
                            usage.input_tokens = u["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                            usage.output_tokens =
                                u["completion_tokens"].as_u64().unwrap_or(0) as u32;
                        }
                    }
                }
            }
        }

        // Assemble tool calls from accumulated deltas
        let mut indices: Vec<u64> = tc_index_map.keys().copied().collect();
        indices.sort();
        for idx in indices {
            if let Some((id, name, args_str)) = tc_index_map.remove(&idx) {
                let arguments: serde_json::Value =
                    serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
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

    fn provider() -> OpenAIProvider {
        OpenAIProvider::new(SecretString::from("test-key".to_string()), "gpt-4o".into())
    }

    #[test]
    fn build_request_body_format() {
        let p = provider();
        let request = ChatRequest {
            system_prompt: Some("Be helpful.".into()),
            messages: vec![ChatMessage::user("Hello")],
            tools: vec![],
            model: None,
            max_tokens: 512,
        };

        let body = p.build_request_body(&request);
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2); // system + user
    }

    #[test]
    fn parse_response_text() {
        let p = provider();
        let body = serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "Hi there!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3}
        });

        let resp = p.parse_response(&body).unwrap();
        assert_eq!(resp.message.content, "Hi there!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 5);
        assert_eq!(resp.usage.output_tokens, 3);
    }

    #[test]
    fn parse_response_tool_calls() {
        let p = provider();
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "search",
                            "arguments": "{\"query\":\"rust\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 8}
        });

        let resp = p.parse_response(&body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].name, "search");
        assert_eq!(resp.message.tool_calls[0].arguments["query"], "rust");
    }

    #[test]
    fn provider_name() {
        let p = provider();
        assert_eq!(p.name(), "openai");
    }
}
