//! Anthropic Messages 协议适配器（含 GLM Anthropic 协议端点）。

use crate::{
    rounded_temperature, ChatMessage, ChatRequest, ChatResponse, ChatStream, ChatStreamEvent,
    ModelProvider, ProviderError, ProviderResult, Role, ToolCallReq, Usage,
};
use futures::StreamExt;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::pin::Pin;

pub struct AnthropicProvider {
    name: String,
    base_url: String,
    api_key: String,
    default_model: Option<String>,
    client: reqwest::Client,
    /// 流式专用：总超时会剪断长生成（reqwest 的 timeout 覆盖到响应体读完），
    /// 改为连接 + 空闲读超时
    stream_client: reqwest::Client,
    /// v1.173 §11 Prompt caching：请求体打 `cache_control: ephemeral` 断点
    /// （system + 最后一条消息）；命中量经 `cache_read_input_tokens` 记账。
    prompt_caching: bool,
}

const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Default, Clone)]
struct StreamedToolBlock {
    id: String,
    name: String,
    arguments: String,
}

struct AnthropicSseState {
    body: Option<Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>>,
    line_buffer: Vec<u8>,
    queue: VecDeque<ChatStreamEvent>,
    content: String,
    tool_blocks: BTreeMap<u64, StreamedToolBlock>,
    usage: Usage,
    model: String,
    finish_reason: Option<String>,
    done: bool,
}

impl AnthropicSseState {
    fn new(response: reqwest::Response, model: String) -> Self {
        Self {
            body: Some(Box::pin(response.bytes_stream())),
            line_buffer: Vec::new(),
            queue: VecDeque::new(),
            content: String::new(),
            tool_blocks: BTreeMap::new(),
            usage: Usage::default(),
            model,
            finish_reason: None,
            done: false,
        }
    }

    fn consume_sse_line(&mut self, line: &str) -> ProviderResult<()> {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            return Ok(());
        };
        let v: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| ProviderError::Parse(format!("invalid stream chunk: {e}")))?;
        match v.get("type").and_then(|t| t.as_str()) {
            Some("message_start") => {
                let message = v.get("message").cloned().unwrap_or_default();
                if let Some(model) = message.get("model").and_then(|m| m.as_str()) {
                    self.model = model.to_string();
                }
                self.usage.input_tokens = message
                    .pointer("/usage/input_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(self.usage.input_tokens);
                self.usage.cached_input_tokens = message
                    .pointer("/usage/cache_read_input_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(self.usage.cached_input_tokens);
            }
            Some("content_block_start") => {
                let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or_default();
                let block = v.get("content_block").cloned().unwrap_or_default();
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let item = self.tool_blocks.entry(index).or_default();
                    item.id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or_default()
                        .to_string();
                    item.name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string();
                }
            }
            Some("content_block_delta") => {
                let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or_default();
                let delta = v.get("delta").cloned().unwrap_or_default();
                match delta.get("type").and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            self.content.push_str(text);
                            self.queue
                                .push_back(ChatStreamEvent::Delta(text.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(text) = delta.get("partial_json").and_then(|t| t.as_str()) {
                            self.tool_blocks
                                .entry(index)
                                .or_default()
                                .arguments
                                .push_str(text);
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                self.finish_reason = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|s| s.as_str())
                    .map(String::from);
                self.usage.output_tokens = v
                    .pointer("/usage/output_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(self.usage.output_tokens);
                self.usage.cached_input_tokens = v
                    .pointer("/usage/cache_read_input_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(self.usage.cached_input_tokens);
            }
            Some("message_stop") => self.done = true,
            _ => {}
        }
        Ok(())
    }

    fn final_response(&self) -> ProviderResult<ChatResponse> {
        let mut tool_calls = Vec::with_capacity(self.tool_blocks.len());
        for (index, block) in &self.tool_blocks {
            let arguments = if block.arguments.trim().is_empty() {
                serde_json::Value::Object(Default::default())
            } else {
                serde_json::from_str(&block.arguments).map_err(|e| {
                    ProviderError::Parse(format!("tool block [{index}] arguments: {e}"))
                })?
            };
            tool_calls.push(ToolCallReq {
                id: block.id.clone(),
                name: block.name.clone(),
                arguments,
            });
        }
        Ok(ChatResponse {
            content: self.content.clone(),
            tool_calls,
            usage: self.usage,
            model: self.model.clone(),
            finish_reason: self.finish_reason.clone(),
        })
    }
}

impl AnthropicProvider {
    pub fn new(name: &str, base_url: &str, api_key: &str, default_model: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            default_model,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("reqwest client"),
            stream_client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .read_timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("reqwest stream client"),
            prompt_caching: true,
        }
    }

    /// v1.173：关闭 Anthropic prompt caching（`[models.caching].enabled=false`）。
    pub fn with_prompt_caching(mut self, enabled: bool) -> Self {
        self.prompt_caching = enabled;
        self
    }

    fn to_api_messages(messages: &[ChatMessage]) -> (String, Vec<serde_json::Value>) {
        let mut system = String::new();
        let mut out = Vec::new();
        for m in messages {
            match m.role {
                Role::System => {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&m.content);
                }
                Role::User => {
                    // v1.191 §11：带图消息先 image 块（base64 source）后 text 块
                    let mut blocks: Vec<serde_json::Value> = m
                        .images
                        .iter()
                        .map(|img| {
                            serde_json::json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": img.media_type,
                                    "data": img.data_base64,
                                }
                            })
                        })
                        .collect();
                    blocks.push(serde_json::json!({ "type": "text", "text": m.content }));
                    out.push(serde_json::json!({ "role": "user", "content": blocks }))
                }
                Role::Assistant => {
                    let mut blocks = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(serde_json::json!({ "type": "text", "text": m.content }));
                    }
                    for tc in &m.tool_calls {
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                    out.push(serde_json::json!({ "role": "assistant", "content": blocks }));
                }
                Role::Tool => {
                    // tool 结果在 Anthropic 协议中是 user 角色的 tool_result 块
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                            "content": m.content,
                        }],
                    }));
                }
            }
        }
        (system, out)
    }

    /// v1.173 §11 Prompt caching：system 块与最后一条消息打 `cache_control:
    /// ephemeral` 断点。system 从纯串改 blocks 数组（缓存工具定义 + 系统提示
    /// 前缀）；最后一条消息的最后一个内容块追加断点——增量式会话前缀缓存，
    /// 断点随回合前移、下回合命中（2 个断点，Anthropic 上限 4 之内）。
    fn apply_cache_control(&self, body: &mut serde_json::Value) {
        if !self.prompt_caching {
            return;
        }
        if let Some(system) = body.get_mut("system") {
            let text = system.as_str().unwrap_or_default().to_string();
            *system = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": { "type": "ephemeral" },
            }]);
        }
        if let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            if let Some(last) = msgs.last_mut() {
                if let Some(blocks) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                    if let Some(last_block) = blocks.last_mut() {
                        last_block["cache_control"] = serde_json::json!({ "type": "ephemeral" });
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> String {
        self.default_model.clone().unwrap_or_default()
    }

    async fn chat(&self, req: &ChatRequest) -> ProviderResult<ChatResponse> {
        let (system, messages) = Self::to_api_messages(&req.messages);
        let mut body = serde_json::json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "temperature": rounded_temperature(req.temperature),
            "messages": messages,
        });
        // 标题等低档推理调用关闭扩展思考；GLM Anthropic 端点实测在
        // enabled-by-default 下会用 128 token 只产出 thinking。档位集合与
        // OpenAI 兼容路径 apply_reasoning 同规（low/minimal/none）。
        if matches!(
            req.reasoning_effort.as_deref(),
            Some("low") | Some("minimal") | Some("none")
        ) {
            body["thinking"] = serde_json::json!({"type": "disabled"});
        }
        if !system.is_empty() {
            body["system"] = serde_json::json!(system);
        }
        if !req.tools.is_empty() {
            body["tools"] = serde_json::json!(req
                .tools
                .iter()
                .map(|t| serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                }))
                .collect::<Vec<_>>());
        }

        let url = format!("{}/v1/messages", self.base_url);
        self.apply_cache_control(&mut body);
        let mut request = self
            .client
            .post(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body);
        if !self.api_key.is_empty() {
            request = request.header("x-api-key", &self.api_key);
        }
        let resp = request
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        // 429 在消费 body 前先取 Retry-After 头（v1.171 §9.1 自动恢复退避依据）
        let retry_after = if status == 429 {
            ProviderError::retry_after_from_headers(resp.headers())
        } else {
            None
        };
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if status >= 400 {
            return Err(match status {
                429 => ProviderError::RateLimited {
                    body: text,
                    retry_after,
                },
                _ => ProviderError::Http { status, body: text },
            });
        }
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| ProviderError::Parse(e.to_string()))?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        if let Some(blocks) = v.get("content").and_then(|c| c.as_array()) {
            for b in blocks {
                match b.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            content.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        tool_calls.push(ToolCallReq {
                            id: b
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string(),
                            name: b
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string(),
                            arguments: b.get("input").cloned().unwrap_or_default(),
                        });
                    }
                    _ => {}
                }
            }
        }
        let usage = v.get("usage").cloned().unwrap_or_default();
        Ok(ChatResponse {
            content,
            tool_calls,
            usage: Usage {
                input_tokens: usage
                    .get("input_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(0),
                output_tokens: usage
                    .get("output_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(0),
                cached_input_tokens: usage
                    .get("cache_read_input_tokens")
                    .and_then(|u| u.as_u64())
                    .unwrap_or(0),
            },
            model: v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(&req.model)
                .to_string(),
            finish_reason: v
                .get("stop_reason")
                .and_then(|s| s.as_str())
                .map(String::from),
        })
    }

    async fn chat_stream(&self, req: &ChatRequest) -> ProviderResult<ChatStream> {
        let (system, messages) = Self::to_api_messages(&req.messages);
        let mut body = serde_json::json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "temperature": rounded_temperature(req.temperature),
            "messages": messages,
            "stream": true,
        });
        if !system.is_empty() {
            body["system"] = serde_json::json!(system);
        }
        // 与 chat() 同口径：低档推理调用显式关 thinking，防思考吃光 max_tokens
        // （档位集合与 OpenAI 兼容路径 apply_reasoning 同规）
        if matches!(
            req.reasoning_effort.as_deref(),
            Some("low") | Some("minimal") | Some("none")
        ) {
            body["thinking"] = serde_json::json!({"type": "disabled"});
        }
        if !req.tools.is_empty() {
            body["tools"] = serde_json::json!(req
                .tools
                .iter()
                .map(|t| serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                }))
                .collect::<Vec<_>>());
        }

        let url = format!("{}/v1/messages", self.base_url);
        self.apply_cache_control(&mut body);
        // 流式走 stream_client（无总超时；连接 30s + 空闲读 180s）
        let mut request = self
            .stream_client
            .post(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body);
        if !self.api_key.is_empty() {
            request = request.header("x-api-key", &self.api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = response.status();
        if status.is_client_error() || status.is_server_error() {
            // 429 在消费 body 前先取 Retry-After 头（v1.171 §9.1 自动恢复退避依据）
            let retry_after = if status.as_u16() == 429 {
                ProviderError::retry_after_from_headers(response.headers())
            } else {
                None
            };
            let body = response
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            return Err(if status.as_u16() == 429 {
                ProviderError::RateLimited { body, retry_after }
            } else {
                ProviderError::Http {
                    status: status.as_u16(),
                    body,
                }
            });
        }

        let mut state = AnthropicSseState::new(response, req.model.clone());
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
                yield Err(ProviderError::Network("stream ended before message_stop".into()));
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
