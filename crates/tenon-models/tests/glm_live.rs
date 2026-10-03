//! GLM Coding Plan 真实端点联调测试（读仓库根 config.local.toml；该文件
//! 已 git-ignore、含密钥，不提交源码管理）。文件不存在时自动跳过。

use std::path::PathBuf;

use tenon_models::{
    AnthropicProvider, ChatMessage, ChatRequest, ModelProvider, OpenAiCompatProvider, ToolSpec,
};

struct LocalConfig {
    openai_url: String,
    anthropic_url: String,
    key: String,
    model: String,
}

fn load_local() -> Option<LocalConfig> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config.local.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let v: toml::Value = text.parse().ok()?;
    let providers = v.get("models")?.get("providers")?;
    let glm = providers.get("glm")?;
    let anthropic = providers.get("glm_anthropic")?;
    Some(LocalConfig {
        openai_url: glm.get("base_url")?.as_str()?.to_string(),
        anthropic_url: anthropic.get("base_url")?.as_str()?.to_string(),
        key: glm.get("api_key")?.as_str()?.to_string(),
        model: glm
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("glm-4.6")
            .to_string(),
    })
}

#[tokio::test]
async fn glm_openai_protocol_chat_completion() {
    let Some(cfg) = load_local() else {
        eprintln!("config.local.toml 不存在，跳过联调测试");
        return;
    };
    let provider =
        OpenAiCompatProvider::new("glm", &cfg.openai_url, &cfg.key, Some(cfg.model.clone()));
    let resp = provider
        .chat(&ChatRequest::new(
            &cfg.model,
            vec![
                ChatMessage::system("你是一个简洁的助手。"),
                ChatMessage::user("只回复两个字：你好"),
            ],
        ))
        .await
        .expect("GLM OpenAI 协议调用成功");
    assert!(!resp.content.is_empty(), "应返回非空内容");
    assert!(resp.usage.input_tokens > 0);
    eprintln!("模型: {} 内容: {}", resp.model, resp.content);
}

#[tokio::test]
async fn glm_anthropic_protocol_messages() {
    let Some(cfg) = load_local() else {
        eprintln!("config.local.toml 不存在，跳过联调测试");
        return;
    };
    let provider = AnthropicProvider::new(
        "glm-anthropic",
        &cfg.anthropic_url,
        &cfg.key,
        Some(cfg.model.clone()),
    );
    let resp = provider
        .chat(&ChatRequest::new(
            &cfg.model,
            vec![ChatMessage::user("只回复两个字：你好")],
        ))
        .await
        .expect("GLM Anthropic 协议调用成功");
    assert!(!resp.content.is_empty(), "应返回非空内容");
    eprintln!("模型: {} 内容: {}", resp.model, resp.content);
}

#[tokio::test]
async fn glm_tool_call_roundtrip() {
    let Some(cfg) = load_local() else {
        eprintln!("config.local.toml 不存在，跳过联调测试");
        return;
    };
    let provider =
        OpenAiCompatProvider::new("glm", &cfg.openai_url, &cfg.key, Some(cfg.model.clone()));
    let tools = vec![ToolSpec {
        name: "get_project_name".to_string(),
        description: "获取当前项目的名称。无参数。".to_string(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }];
    let mut req = ChatRequest::new(
        &cfg.model,
        vec![ChatMessage::user("调用工具查询项目名，然后告诉我结果")],
    );
    req.tools = tools;
    let first = provider.chat(&req).await.expect("第一轮调用成功");
    eprintln!(
        "第一轮: content={:?} tool_calls={:?}",
        first.content, first.tool_calls
    );

    // 若模型发起工具调用，回填结果验证闭环
    if !first.tool_calls.is_empty() {
        let call = &first.tool_calls[0];
        let mut messages = req.messages.clone();
        messages.push(ChatMessage::assistant(first.content.clone()));
        let mut assistant = ChatMessage::assistant("");
        assistant.tool_calls = first.tool_calls.clone();
        messages.pop();
        messages.push(assistant);
        messages.push(ChatMessage::tool_result(call.id.clone(), "Tenon（榫）"));
        let mut req2 = ChatRequest::new(&cfg.model, messages);
        req2.tools = vec![ToolSpec {
            name: "get_project_name".to_string(),
            description: "获取当前项目的名称。无参数。".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        let second = provider.chat(&req2).await.expect("第二轮调用成功");
        assert!(
            second.content.contains("Tenon") || second.content.contains("榫"),
            "工具结果应进入上下文，实际: {}",
            second.content
        );
    }
}
