use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tracing::debug;

use aivyx_core::{AivyxError, Result};

use crate::message::{
    ChatMessage, ChatRequest, ChatResponse, Role, StopReason, TokenUsage, ToolCall,
};
use crate::provider::LlmProvider;

/// Ollama provider using the OpenAI-compatible chat completions endpoint.
pub struct OllamaProvider {
    client: Client,
    base_url: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(60))
                .connect_timeout(Duration::from_secs(5))
                .pool_max_idle_per_host(4)
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .expect("failed to build HTTP client"),
            base_url,
            model,
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
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let body = self.build_request_body(request);

        debug!("Sending Ollama API request to {url}");

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AivyxError::Http(format!("Ollama request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::LlmProvider(format!(
                "Ollama API error {status}: {error_body}"
            )));
        }

        let response_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AivyxError::LlmProvider(format!("failed to parse response: {e}")))?;

        self.parse_response(&response_body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_basic() {
        let provider = OllamaProvider::new("http://localhost:11434".into(), "llama3".into());

        let request = ChatRequest {
            system_prompt: Some("Be helpful.".into()),
            messages: vec![ChatMessage::user("Hello")],
            tools: vec![],
            model: None,
            max_tokens: 512,
        };

        let body = provider.build_request_body(&request);
        assert_eq!(body["model"], "llama3");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2); // system + user
    }

    #[test]
    fn parse_response_basic() {
        let provider = OllamaProvider::new("http://localhost:11434".into(), "llama3".into());

        let body = serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "Hi there!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3}
        });

        let resp = provider.parse_response(&body).unwrap();
        assert_eq!(resp.message.content, "Hi there!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }
}
