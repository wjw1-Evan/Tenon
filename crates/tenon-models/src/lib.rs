//! 模型层（设计方案 §11）：BYOK + 显式路由 + OpenAI 兼容端点 + 成本归因。
//!
//! - provider trait + 三类适配器：OpenAI Chat 兼容（覆盖 OpenAI / DeepSeek /
//!   Ollama / GLM 兼容端点）、Anthropic Messages（含 GLM Anthropic 协议端点）、
//!   测试用 mock（脚本化回放）；
//! - 密钥：KeyStore trait（macOS 钥匙串 / 环境变量），不落盘明文；
//! - 路由：v1 显式 + 轻量启发式（纯读任务建议轻模型）；auto 路由实验默认关（§5-8）；
//! - 成本：按价格表计算；本地模型「本地 · 0 成本」。

pub mod anthropic;
pub mod cost;
pub mod keys;
pub mod mock;
pub mod openai;
pub mod routing;

pub use anthropic::AnthropicProvider;
pub use cost::{compute_cost, PriceTable};
pub use keys::{ChainKeyStore, EnvKeyStore, KeyStore, KeychainStore};
pub use mock::{MockProvider, ScriptedReply};
pub use openai::OpenAiCompatProvider;
pub use routing::{is_pure_read_task, Router};

/// 对话标题生成请求的标记（v1.58）：单轮无工具调用的系统提示以此开头，
/// 供测试替身（MockProvider / E2E fake server）识别并返回固定短标题、
/// 不消耗脚本队列——脚本化测试对模型调用次数 / 序列的断言不受自动标题影响。
pub const TITLE_MARKER: &str = "TENON_TASK_TITLE";

/// L5 记忆提取请求的标记（v1.104 §10.1）：任务完成后的单轮无工具提取调用，
/// 系统提示以此开头。测试替身识别后返回固定记忆 JSON、不消耗脚本队列，
/// 与 TITLE_MARKER 同法保证脚本化测试的调用次数 / 序列断言零扰动。
pub const MEMORY_MARKER: &str = "TENON_MEMORY_EXTRACT";

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP 错误 {status}: {body}")]
    Http { status: u16, body: String },
    #[error("网络错误: {0}")]
    Network(String),
    #[error("响应解析失败: {0}")]
    Parse(String),
    #[error("缺少 API Key（provider={0}）")]
    MissingKey(String),
    #[error("配置错误: {0}")]
    Config(String),
}

pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// assistant 消息携带的工具调用请求。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallReq>,
    /// role=tool 时的调用 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: Some(call_id.into()),
        }
    }
}

/// 工具定义（JSON Schema 形式，进 provider 请求）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema parameters。
    pub parameters: serde_json::Value,
}

/// 模型返回的工具调用请求。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallReq {
    pub id: String,
    pub name: String,
    /// JSON 参数对象。
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// 供应商报告的缓存命中输入 token（v1.129 §11）：OpenAI 兼容 =
    /// `prompt_tokens_details.cached_tokens`（prompt 的子集）；Anthropic =
    /// `cache_read_input_tokens`（不含在 input_tokens 内，不打 `cache_control`
    /// 断点恒 0）。未报告 = 0，本地模型 = 0。
    pub cached_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallReq>,
    pub usage: Usage,
    /// 实际服务的模型名（供应商回填）。
    pub model: String,
    /// 是否发生了供应商侧降级（reasoning 模型等）。
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    pub temperature: f32,
    /// 可选推理力度：标题等低复杂度调用用 `low` 关闭/收窄推理，防止
    /// reasoning token 吃光 `max_tokens` 后正文为空。None = 供应商默认。
    pub reasoning_effort: Option<String>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: vec![],
            max_tokens: 4096,
            temperature: 0.2,
            reasoning_effort: None,
        }
    }
}

/// 流式模型回合中的一个事件。
#[derive(Debug, Clone)]
pub enum ChatStreamEvent {
    /// 已可呈现的增量文本。工具调用参数不向用户流式暴露。
    Delta(String),
    /// 当前回合的权威最终响应；usage / tool calls 只以此为准。
    Final(ChatResponse),
}

pub type ChatStream = BoxStream<'static, ProviderResult<ChatStreamEvent>>;

/// 模型供应商抽象。`chat_stream` 是任务回合的首选通道；适配器必须把
/// 最终 `ChatResponse` 作为权威事件放在流末尾（§9.6 / §11）。
#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn chat(&self, req: &ChatRequest) -> ProviderResult<ChatResponse>;
    /// 默认降级为整响应模拟流：非流式后端也能接入同一 Agent / UI 协议。
    async fn chat_stream(&self, req: &ChatRequest) -> ProviderResult<ChatStream> {
        let resp = self.chat(req).await?;
        let text = resp.content.clone();
        let events = std::iter::once(Ok(ChatStreamEvent::Delta(text)))
            .chain(std::iter::once(Ok(ChatStreamEvent::Final(resp))))
            .collect::<Vec<_>>();
        Ok(Box::pin(futures::stream::iter(events)))
    }
    /// 本地模型（Ollama / Laya）：成本恒 0（§11）。
    fn is_local(&self) -> bool {
        false
    }
    /// 该 provider 默认模型（provider 配置 `model` 字段，v1.11）。
    fn default_model(&self) -> String;
}

/// 从 provider 配置构建具体适配器（tenon-config ProviderConfig → 实现）。
pub fn build_provider(
    name: &str,
    cfg: &tenon_config::ProviderConfig,
    keys: &dyn KeyStore,
) -> ProviderResult<std::sync::Arc<dyn ModelProvider>> {
    // 密钥解析链：api_key_env 环境变量 → api_key 直填（仅开发期本地文件）→ 空
    let api_key = cfg
        .api_key_env
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(|env| keys.get(env))
        .or_else(|| cfg.api_key.clone())
        .unwrap_or_default();
    let kind = cfg.kind.unwrap_or_default();
    match kind {
        tenon_config::ProviderKind::Openai => Ok(std::sync::Arc::new(OpenAiCompatProvider::new(
            name,
            &cfg.base_url,
            &api_key,
            cfg.model.clone(),
        ))),
        tenon_config::ProviderKind::Anthropic => Ok(std::sync::Arc::new(AnthropicProvider::new(
            name,
            &cfg.base_url,
            &api_key,
            cfg.model.clone(),
        ))),
        tenon_config::ProviderKind::OpenaiResponses => {
            // Responses 协议随 M1 落地；当前以 chat 协议兼容尝试（GLM/OpenAI 均提供 chat）。
            Ok(std::sync::Arc::new(OpenAiCompatProvider::new(
                name,
                &cfg.base_url,
                &api_key,
                cfg.model.clone(),
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn build_provider_from_config_kinds() {
        let mut providers = BTreeMap::new();
        providers.insert(
            "glm".to_string(),
            tenon_config::ProviderConfig {
                kind: Some(tenon_config::ProviderKind::Anthropic),
                base_url: "https://example.com/api/anthropic".into(),
                wire_api: None,
                api_key_env: Some("TENON_TEST_KEY".into()),
                api_key: None,
                model: Some("glm-4.6".into()),
                price_in_per_mtok: None,
                price_out_per_mtok: None,
            },
        );
        let cfg = tenon_config::ModelsConfig {
            default: "glm".into(),
            providers,
            ..Default::default()
        };
        std::env::set_var("TENON_TEST_KEY", "k-123");
        let keys = EnvKeyStore;
        let p = cfg.providers.get("glm").unwrap();
        let built = build_provider("glm", p, &keys).unwrap();
        assert_eq!(built.name(), "glm");
        assert_eq!(built.default_model(), "glm-4.6");
        assert!(!built.is_local());
    }
}
