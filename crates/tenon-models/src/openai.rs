//! OpenAI Chat Completion 兼容适配器：覆盖 OpenAI / DeepSeek / Ollama /
//! GLM 等兼容端点（§11 通用 provider；config schema 借鉴 codex，§3.1）。

use crate::{
    ChatMessage, ChatRequest, ChatResponse, ModelProvider, ProviderError, ProviderResult, Role,
    ToolCallReq, Usage,
};

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
}

/// 兼容旧引用（chat 消息类型导出）。
pub type Msg = ChatMessage;
