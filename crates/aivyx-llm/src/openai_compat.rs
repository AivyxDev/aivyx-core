use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use aivyx_core::{AivyxError, Result};

use crate::message::{
    ChatMessage, ChatRequest, ChatResponse, Content, ContentBlock, ImageSource, Role, StopReason,
    TokenUsage, ToolCall,
};
use crate::provider::{LlmProvider, StreamEvent};

/// Generic provider for any service implementing the OpenAI Chat Completions API.
///
/// Works with OpenAI, Groq, Together AI, Mistral, DeepSeek, OpenRouter, xAI,
/// Ollama, vLLM, LM Studio, and any other OpenAI-compatible endpoint.
pub struct OpenAICompatibleProvider {
    client: Client,
    api_key: Option<SecretString>,
    model: String,
    api_url: String,
    provider_name: String,
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider.
    ///
    /// `base_url` is the server root (e.g. `https://api.groq.com/openai`);
    /// `/v1/chat/completions` is appended automatically.
    /// `api_key` is optional — local servers like Ollama or vLLM may not need one.
    pub fn new(
        api_key: Option<SecretString>,
        model: String,
        base_url: String,
        provider_name: String,
        timeout_secs: u64,
    ) -> Self {
        let api_url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
        Self {
            client: Client::builder()
                // Use read_timeout instead of timeout: `timeout()` is a total
                // request lifetime limit which kills long-running LLM inference
                // (e.g. 32B models taking 300-500s). `read_timeout()` resets on
                // each received chunk, so streaming inference stays alive as
                // long as tokens are being generated.
                .read_timeout(Duration::from_secs(timeout_secs))
                .connect_timeout(Duration::from_secs(10))
                .pool_max_idle_per_host(4)
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .expect("failed to build HTTP client"),
            api_key,
            model,
            api_url,
            provider_name,
        }
    }

    pub(crate) fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
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
                        "content": msg.content.text(),
                    }));
                }
                Role::User => {
                    let api_content = match &msg.content {
                        Content::Text(s) => serde_json::json!(s),
                        Content::Blocks(blocks) => {
                            let mapped: Vec<serde_json::Value> = blocks
                                .iter()
                                .map(|b| match b {
                                    ContentBlock::Text { text } => {
                                        serde_json::json!({"type": "text", "text": text})
                                    }
                                    ContentBlock::Image { source } => match source {
                                        ImageSource::Base64 { media_type, data } => {
                                            serde_json::json!({
                                                "type": "image_url",
                                                "image_url": {
                                                    "url": format!("data:{media_type};base64,{data}")
                                                }
                                            })
                                        }
                                        ImageSource::Url { url } => {
                                            serde_json::json!({
                                                "type": "image_url",
                                                "image_url": {"url": url}
                                            })
                                        }
                                    },
                                })
                                .collect();
                            serde_json::json!(mapped)
                        }
                    };
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": api_content,
                    }));
                }
                Role::Assistant => {
                    let mut entry = serde_json::json!({
                        "role": "assistant",
                        "content": msg.content.text(),
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

    pub(crate) fn parse_response(&self, body: &serde_json::Value) -> Result<ChatResponse> {
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
                let id = tc["id"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
                    AivyxError::LlmProvider("tool_call missing 'id' field".into())
                })?;
                let name = function["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        AivyxError::LlmProvider("tool_call missing 'function.name' field".into())
                    })?;
                let args_str = function["arguments"].as_str().unwrap_or("{}");
                let arguments: serde_json::Value = serde_json::from_str(args_str).map_err(|e| {
                    warn!(
                        tool = %name,
                        raw_args = %args_str,
                        "failed to parse tool arguments"
                    );
                    AivyxError::LlmProvider(format!("malformed tool arguments for '{name}': {e}"))
                })?;
                tool_calls.push(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
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

    /// Build a request with optional auth header.
    fn build_http_request(&self, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(&self.api_url)
            .header("content-type", "application/json")
            .json(body);

        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key.expose_secret()));
        }

        req
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let body = self.build_request_body(request);

        debug!("Sending {} API request", self.provider_name);

        let response = self.build_http_request(&body).send().await.map_err(|e| {
            AivyxError::Http(format!("{} API request failed: {e}", self.provider_name))
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AivyxError::RateLimit(format!(
                "{} API rate limit exceeded",
                self.provider_name
            )));
        }

        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::LlmProvider(format!(
                "{} API error {status}: {error_body}",
                self.provider_name
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

        debug!("Sending {} API streaming request", self.provider_name);

        let response = self.build_http_request(&body).send().await.map_err(|e| {
            AivyxError::Http(format!("{} API request failed: {e}", self.provider_name))
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AivyxError::RateLimit(format!(
                "{} API rate limit exceeded",
                self.provider_name
            )));
        }
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AivyxError::LlmProvider(format!(
                "{} API error {status}: {error_body}",
                self.provider_name
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

        // 10 MB guard — prevents memory exhaustion from a malicious/broken upstream
        const MAX_BUFFER_SIZE: usize = 10 * 1024 * 1024;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AivyxError::Http(format!("stream read error: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            if buffer.len() > MAX_BUFFER_SIZE {
                return Err(AivyxError::LlmProvider(
                    "SSE stream buffer exceeded 10 MB — aborting".into(),
                ));
            }

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

                        // Usage (OpenAI includes usage in the final chunk when
                        // stream_options.include_usage is set)
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
                    serde_json::from_str(&args_str).map_err(|e| {
                        warn!(
                            tool = %name,
                            raw_args = %args_str,
                            "failed to parse streamed tool arguments"
                        );
                        AivyxError::LlmProvider(format!(
                            "malformed tool arguments for '{name}': {e}"
                        ))
                    })?;
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

    fn provider() -> OpenAICompatibleProvider {
        OpenAICompatibleProvider::new(
            Some(SecretString::from("test-key".to_string())),
            "gpt-4o".into(),
            "https://api.openai.com".into(),
            "test-provider".into(),
            120,
        )
    }

    fn provider_no_auth() -> OpenAICompatibleProvider {
        OpenAICompatibleProvider::new(
            None,
            "llama3".into(),
            "http://localhost:11434".into(),
            "local".into(),
            60,
        )
    }

    #[test]
    fn api_url_constructed_correctly() {
        let p = provider();
        assert_eq!(p.api_url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn api_url_strips_trailing_slash() {
        let p = OpenAICompatibleProvider::new(
            None,
            "model".into(),
            "http://localhost:8080/".into(),
            "test".into(),
            60,
        );
        assert_eq!(p.api_url, "http://localhost:8080/v1/chat/completions");
    }

    #[test]
    fn provider_name_returned() {
        assert_eq!(provider().name(), "test-provider");
        assert_eq!(provider_no_auth().name(), "local");
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
    fn build_request_body_model_override() {
        let p = provider();
        let request = ChatRequest {
            system_prompt: None,
            messages: vec![ChatMessage::user("Hi")],
            tools: vec![],
            model: Some("gpt-4o-mini".into()),
            max_tokens: 100,
        };

        let body = p.build_request_body(&request);
        assert_eq!(body["model"], "gpt-4o-mini");
    }

    #[test]
    fn build_request_body_with_tools() {
        let p = provider();
        let request = ChatRequest {
            system_prompt: None,
            messages: vec![ChatMessage::user("Search for rust")],
            tools: vec![serde_json::json!({
                "name": "web_search",
                "description": "Search the web",
                "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}}
            })],
            model: None,
            max_tokens: 512,
        };

        let body = p.build_request_body(&request);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "web_search");
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
        assert_eq!(resp.message.content.text(), "Hi there!");
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
    fn parse_response_max_tokens() {
        let p = provider();
        let body = serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "truncated"},
                "finish_reason": "length"
            }]
        });

        let resp = p.parse_response(&body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn parse_response_no_usage() {
        let p = provider();
        let body = serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }]
        });

        let resp = p.parse_response(&body).unwrap();
        assert_eq!(resp.usage.input_tokens, 0);
        assert_eq!(resp.usage.output_tokens, 0);
    }

    #[test]
    fn no_auth_provider_works() {
        let p = provider_no_auth();
        let request = ChatRequest {
            system_prompt: None,
            messages: vec![ChatMessage::user("Hello")],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };

        // Verify body builds correctly without auth
        let body = p.build_request_body(&request);
        assert_eq!(body["model"], "llama3");
    }

    #[test]
    fn multimodal_user_message() {
        let p = provider();
        let msg = ChatMessage::user_with_images(
            "describe this",
            vec![ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "abc123".into(),
            }],
        );
        let request = ChatRequest {
            system_prompt: None,
            messages: vec![msg],
            tools: vec![],
            model: None,
            max_tokens: 100,
        };

        let body = p.build_request_body(&request);
        let user_content = &body["messages"][0]["content"];
        let blocks = user_content.as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image_url");
    }
}
