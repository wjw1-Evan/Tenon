//! OpenAI Chat Completion 兼容适配器：覆盖 OpenAI / DeepSeek / Ollama /
//! GLM 等兼容端点（§11 通用 provider；config schema 借鉴 codex，§3.1）。

use crate::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, ChatStreamEvent, ModelProvider,
    ProviderError, ProviderResult, Role, ToolCallReq, Usage,
};
use futures::StreamExt;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::pin::Pin;

pub struct OpenAiCompatProvider {
    name: String,
    base_url: String,
    api_key: String,
    default_model: Option<String>,
    client: reqwest::Client,
    local: bool,
}

impl OpenAiCompatProvider {
    pub fn new(name: &str, base_url: &str, api_key: &str, default_model: Option<String>) -> Self {
        let local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
        Self {
            name: name.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            default_model,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("reqwest client"),
            local,
        }
    }

    fn map_role(role: Role) -> &'static str {
        match role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Default, Clone)]
struct StreamedToolCall {
    id: String,
    name: String,
    arguments: String,
}

struct OpenAiSseState {
    body: Option<Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>>,
    line_buffer: Vec<u8>,
    queue: VecDeque<ChatStreamEvent>,
    content: String,
    reasoning: String,
    tool_calls: BTreeMap<u64, StreamedToolCall>,
    usage: Usage,
    model: String,
    finish_reason: Option<String>,
    done: bool,
}

impl OpenAiSseState {
    fn new(response: reqwest::Response, model: String) -> Self {
        Self {
            body: Some(Box::pin(response.bytes_stream())),
            line_buffer: Vec::new(),
            queue: VecDeque::new(),
            content: String::new(),
            reasoning: String::new(),
            tool_calls: BTreeMap::new(),
            usage: Usage::default(),
            model,
            finish_reason: None,
            done: false,
        }
    }

    fn consume_sse_line(&mut self, line: &str) -> ProviderResult<()> {
        let data = line.strip_prefix("data:").map(str::trim);
        let Some(data) = data else {
            return Ok(());
        };
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let v: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| ProviderError::Parse(format!("invalid stream chunk: {e}")))?;
        if let Some(model) = v.get("model").and_then(|m| m.as_str()) {
            self.model = model.to_string();
        }
        if let Some(usage) = v.get("usage") {
            self.usage.input_tokens = usage
                .get("prompt_tokens")
                .and_then(|u| u.as_u64())
                .unwrap_or(self.usage.input_tokens);
            self.usage.output_tokens = usage
                .get("completion_tokens")
                .and_then(|u| u.as_u64())
                .unwrap_or(self.usage.output_tokens);
        }
        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
            return Ok(());
        };
        if let Some(reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            self.finish_reason = Some(reason.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(());
        };
        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            if !text.is_empty() {
                self.content.push_str(text);
                self.queue
                    .push_back(ChatStreamEvent::Delta(text.to_string()));
            }
        }
        // 部分 OpenAI 兼容推理模型只在 reasoning_content 提供正文。
        if let Some(text) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
            if !text.is_empty() {
                self.reasoning.push_str(text);
                self.queue
                    .push_back(ChatStreamEvent::Delta(text.to_string()));
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for call in calls {
                let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                let item = self.tool_calls.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                    if !id.is_empty() {
                        item.id = id.to_string();
                    }
                }
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                        if !name.is_empty() {
                            item.name.push_str(name);
                        }
                    }
                    if let Some(args) = function.get("arguments").and_then(|a| a.as_str()) {
                        item.arguments.push_str(args);
                    }
                }
            }
        }
        Ok(())
    }

    fn final_response(&self) -> ProviderResult<ChatResponse> {
        let mut tool_calls = Vec::with_capacity(self.tool_calls.len());
        for (index, call) in &self.tool_calls {
            let arguments = if call.arguments.trim().is_empty() {
                serde_json::Value::Object(Default::default())
            } else {
                serde_json::from_str(&call.arguments).map_err(|e| {
                    ProviderError::Parse(format!("tool call [{index}] arguments: {e}"))
                })?
            };
            tool_calls.push(ToolCallReq {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments,
            });
        }
        let mut content = self.content.clone();
        if content.is_empty() {
            content = self.reasoning.clone();
        }
        Ok(ChatResponse {
            content,
            tool_calls,
            usage: self.usage,
            model: self.model.clone(),
            finish_reason: self.finish_reason.clone(),
        })
    }
}

#[async_trait::async_trait]
impl ModelProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_local(&self) -> bool {
        self.local
    }

    fn default_model(&self) -> String {
        self.default_model.clone().unwrap_or_default()
    }

    async fn chat(&self, req: &ChatRequest) -> ProviderResult<ChatResponse> {
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": req.messages.iter().map(|m| {
                let mut j = serde_json::json!({
                    "role": Self::map_role(m.role),
                    "content": m.content,
                });
                if m.role == Role::Assistant && !m.tool_calls.is_empty() {
                    j["tool_calls"] = serde_json::json!(m.tool_calls.iter().map(|tc| serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": { "name": tc.name, "arguments": tc.arguments.to_string() },
                    })).collect::<Vec<_>>());
                }
                if m.role == Role::Tool {
                    j["tool_call_id"] = serde_json::json!(m.tool_call_id.clone().unwrap_or_default());
                }
                j
            }).collect::<Vec<_>>(),
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
        });
        if !req.tools.is_empty() {
            body["tools"] = serde_json::json!(req
                .tools
                .iter()
                .map(|t| serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                }))
                .collect::<Vec<_>>());
        }

        let url = format!("{}/chat/completions", self.base_url);
        let mut request = self.client.post(&url).json(&body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let resp = request
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if status >= 400 {
            return Err(ProviderError::Http { status, body: text });
        }
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| ProviderError::Parse(e.to_string()))?;

        let choice = v
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or_else(|| ProviderError::Parse(format!("missing choices: {text}")))?;
        let message = choice.get("message").cloned().unwrap_or_default();

        // 部分推理模型把回答放在 reasoning_content，content 为空（GLM 实测）；
        // 无 tool_calls 且 content 空时回退展示 reasoning 摘要。
        let mut content = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if content.is_empty() {
            if let Some(rc) = message.get("reasoning_content").and_then(|c| c.as_str()) {
                content = rc.to_string();
            }
        }
        let tool_calls: Vec<ToolCallReq> = message
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        let func = tc.get("function")?;
                        Some(ToolCallReq {
                            id: tc.get("id")?.as_str()?.to_string(),
                            name: func.get("name")?.as_str()?.to_string(),
                            arguments: func
                                .get("arguments")
                                .and_then(|a| a.as_str())
                                .and_then(|s| serde_json::from_str(s).ok())
                                .unwrap_or(serde_json::Value::Object(Default::default())),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = v.get("usage").cloned().unwrap_or_default();
        Ok(ChatResponse {
            content,
            tool_calls,
            usage: Usage {
                input_tokens: usage
                    .get("prompt_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(0),
                output_tokens: usage
                    .get("completion_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(0),
            },
            model: v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(&req.model)
                .to_string(),
            finish_reason: choice
                .get("finish_reason")
                .and_then(|f| f.as_str())
                .map(String::from),
        })
    }

    async fn chat_stream(&self, req: &ChatRequest) -> ProviderResult<ChatStream> {
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": req.messages.iter().map(|m| {
                let mut j = serde_json::json!({
                    "role": Self::map_role(m.role),
                    "content": m.content,
                });
                if m.role == Role::Assistant && !m.tool_calls.is_empty() {
                    j["tool_calls"] = serde_json::json!(m.tool_calls.iter().map(|tc| serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": { "name": tc.name, "arguments": tc.arguments.to_string() },
                    })).collect::<Vec<_>>());
                }
                if m.role == Role::Tool {
                    j["tool_call_id"] = serde_json::json!(m.tool_call_id.clone().unwrap_or_default());
                }
                j
            }).collect::<Vec<_>>(),
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if !req.tools.is_empty() {
            body["tools"] = serde_json::json!(req
                .tools
                .iter()
                .map(|t| serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                }))
                .collect::<Vec<_>>());
        }

        let url = format!("{}/chat/completions", self.base_url);
        let mut request = self.client.post(&url).json(&body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = response.status();
        if status.is_client_error() || status.is_server_error() {
            let body = response
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            return Err(ProviderError::Http {
                status: status.as_u16(),
                body,
            });
        }

        let mut state = OpenAiSseState::new(response, req.model.clone());
        let stream = async_stream::stream! {
            while let Some(event) = state.queue.pop_front() {
                yield Ok(event);
            }
            let Some(mut bytes) = state.body.take() else {
                yield Err(ProviderError::Network("response stream already consumed".into()));
                return;
            };
            while let Some(chunk_result) = bytes.next().await {
                let chunk = match chunk_result {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        yield Err(ProviderError::Network(error.to_string()));
                        return;
                    }
                };
                state.line_buffer.extend_from_slice(&chunk);
                while let Some(pos) = state.line_buffer.iter().position(|byte| *byte == b'\n') {
                    let line_bytes: Vec<u8> = state.line_buffer.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
                    if let Err(error) = state.consume_sse_line(line.trim_end_matches('\r')) {
                        yield Err(error);
                        return;
                    }
                }
                while let Some(event) = state.queue.pop_front() {
                    yield Ok(event);
                }
                if state.done {
                    break;
                }
            }
            state.body = Some(bytes);
            if !state.line_buffer.is_empty() {
                let line = String::from_utf8_lossy(&state.line_buffer).to_string();
                state.line_buffer.clear();
                if let Err(error) = state.consume_sse_line(line.trim()) {
                    yield Err(error);
                    return;
                }
            }
            if !state.done {
                yield Err(ProviderError::Network("stream ended before [DONE]".into()));
                return;
            }
            if state.queue.is_empty() {
                match state.final_response() {
                    Ok(resp) => state.queue.push_back(ChatStreamEvent::Final(resp)),
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                }
            }
            while let Some(event) = state.queue.pop_front() {
                yield Ok(event);
            }
        };
        Ok(Box::pin(stream))
    }
}

/// 兼容旧引用（chat 消息类型导出）。
pub type Msg = ChatMessage;
