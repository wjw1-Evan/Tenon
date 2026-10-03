//! 测试 / Evals 用脚本化 mock provider（附录 D 基准任务无云端依赖运行）。

use crate::{ChatRequest, ChatResponse, ModelProvider, ProviderResult, ToolCallReq, Usage};

/// 一条脚本化回复。
#[derive(Debug, Clone)]
pub enum ScriptedReply {
    /// 纯文本回答（无改动路径）。
    Text(String),
    /// 工具调用（参数为 JSON）。
    Tool {
        name: String,
        args: serde_json::Value,
    },
    /// 文本 + 工具调用混合。
    Mixed {
        text: String,
        tool: (String, serde_json::Value),
    },
    /// 模拟供应商故障（触发 ERROR / model_fallback 路径测试）。
    Failure(String),
}

pub struct MockProvider {
    name: String,
    model: String,
    script: std::sync::Mutex<Vec<ScriptedReply>>,
    /// 脚本耗尽后循环最后一个回复（默认 true，避免长会话中断）。
    loop_last: bool,
    calls: std::sync::Mutex<Vec<ChatRequest>>,
}

impl MockProvider {
    pub fn new(name: &str, model: &str, script: Vec<ScriptedReply>) -> Self {
        Self {
            name: name.to_string(),
            model: model.to_string(),
            script: std::sync::Mutex::new(script),
            loop_last: true,
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<ChatRequest> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait::async_trait]
impl ModelProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> String {
        self.model.clone()
    }

    async fn chat(&self, req: &ChatRequest) -> ProviderResult<ChatResponse> {
        // 输入 token 与真实 prompt 规模成正比（≈3 字符/token）：
        // 让 Evals 门（§18.3「token 下降」）可离线度量
        let prompt_chars: usize = req
            .messages
            .iter()
            .map(|m| {
                m.content.chars().count()
                    + m.tool_calls
                        .iter()
                        .map(|tc| tc.name.len() + tc.arguments.to_string().len())
                        .sum::<usize>()
            })
            .sum::<usize>()
            + req
                .tools
                .iter()
                .map(|t| t.description.len() + 24)
                .sum::<usize>();
        let input_tokens = (prompt_chars as u64 / 3).max(1);
        self.calls.lock().expect("calls lock").push(req.clone());
        let next = {
            let mut script = self.script.lock().expect("script lock");
            if script.is_empty() {
                None
            } else if script.len() == 1 && self.loop_last {
                Some(script[0].clone())
            } else {
                Some(script.remove(0))
            }
        };
        let reply = next.ok_or_else(|| crate::ProviderError::Config("mock 脚本已耗尽".into()))?;
        let resp = match reply {
            ScriptedReply::Text(t) => ChatResponse {
                content: t,
                tool_calls: vec![],
                usage: Usage {
                    input_tokens,
                    output_tokens: 5,
                },
                model: self.model.clone(),
                finish_reason: Some("stop".into()),
            },
            ScriptedReply::Tool { name, args } => ChatResponse {
                content: String::new(),
                tool_calls: vec![ToolCallReq {
                    id: format!("call_{}", self.calls.lock().unwrap().len()),
                    name,
                    arguments: args,
                }],
                usage: Usage {
                    input_tokens,
                    output_tokens: 5,
                },
                model: self.model.clone(),
                finish_reason: Some("tool_use".into()),
            },
            ScriptedReply::Mixed { text, tool } => ChatResponse {
                content: text,
                tool_calls: vec![ToolCallReq {
                    id: format!("call_{}", self.calls.lock().unwrap().len()),
                    name: tool.0,
                    arguments: tool.1,
                }],
                usage: Usage {
                    input_tokens,
                    output_tokens: 5,
                },
                model: self.model.clone(),
                finish_reason: Some("tool_use".into()),
            },
            ScriptedReply::Failure(msg) => {
                return Err(crate::ProviderError::Network(msg));
            }
        };
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scripted_tool_then_text() {
        let mock = MockProvider::new(
            "mock",
            "mock-1",
            vec![
                ScriptedReply::Tool {
                    name: "list_dir".to_string(),
                    args: serde_json::json!({"path": "."}),
                },
                ScriptedReply::Text("done".into()),
            ],
        );
        let r1 = mock
            .chat(&ChatRequest::new("mock-1", vec![]))
            .await
            .unwrap();
        assert_eq!(r1.tool_calls.len(), 1);
        assert_eq!(r1.tool_calls[0].name, "list_dir");

        let r2 = mock
            .chat(&ChatRequest::new("mock-1", vec![]))
            .await
            .unwrap();
        assert_eq!(r2.content, "done");
        // 输入 token ∝ prompt 规模（空请求也有 ≥1）
        assert!(r2.usage.input_tokens >= 1);
        assert_eq!(r2.usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn failure_reply_maps_to_provider_error() {
        let mock = MockProvider::new("mock", "m", vec![ScriptedReply::Failure("boom".into())]);
        let err = mock.chat(&ChatRequest::new("m", vec![])).await.unwrap_err();
        assert!(err.to_string().contains("boom"));
    }
}
