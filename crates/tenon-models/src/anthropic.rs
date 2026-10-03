//! Anthropic Messages 协议适配器（含 GLM Anthropic 协议端点）。

use crate::{
    ChatMessage, ChatRequest, ChatResponse, ModelProvider, ProviderError, ProviderResult, Role,
    ToolCallReq, Usage,
};

pub struct AnthropicProvider {
    name: String,
    base_url: String,
    api_key: String,
    default_model: Option<String>,
    client: reqwest::Client,
}

const ANTHROPIC_VERSION: &str = "2023-06-01";

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
        }
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
                Role::User => out.push(serde_json::json!({
                    "role": "user",
                    "content": [{ "type": "text", "text": m.content }],
                })),
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
            "temperature": req.temperature,
            "messages": messages,
        });
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
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if status >= 400 {
            return Err(ProviderError::Http { status, body: text });
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
}
