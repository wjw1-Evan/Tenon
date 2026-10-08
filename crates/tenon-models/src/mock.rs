//! 测试 / Evals 用脚本化 mock provider（附录 D 基准任务无云端依赖运行）。

use crate::{
    ChatRequest, ChatResponse, ChatStream, ChatStreamEvent, ModelProvider, ProviderResult,
    ToolCallReq, Usage,
};

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
    /// 输出被 max_tokens 截断（finish_reason=length，无工具调用）——v1.53 截断续跑。
    Truncated(String),
    /// 模拟供应商故障（触发 ERROR / model_fallback 路径测试）。
    Failure(String),
    /// 模拟上游限流 429（v1.171 §9.1 自动恢复：瞬时错误，retry_after_ms 模拟
    /// Retry-After 头——测试用毫秒级退避把用例压进亚秒）。
    RateLimited { retry_after_ms: Option<u64> },
    /// 模拟非瞬时故障（401 认证失败等，v1.171：不重试直接 fallback 链）。
    NonTransient(String),
    /// 模拟上下文超限 400（v1.191 §10.2 溢出恢复：非瞬时，会话侧压缩后重试一次）。
    ContextOverflow,
}

pub struct MockProvider {
    name: String,
    model: String,
    script: std::sync::Mutex<Vec<ScriptedReply>>,
    /// 脚本耗尽后循环最后一个回复（默认 true，避免长会话中断）。
    loop_last: bool,
    calls: std::sync::Mutex<Vec<ChatRequest>>,
    /// 标题生成请求（TITLE_MARKER，v1.58）单独记录，不进 calls / 脚本队列。
    title_calls: std::sync::Mutex<Vec<ChatRequest>>,
    /// 记忆提取请求（MEMORY_MARKER，v1.104）单独记录，不进 calls / 脚本队列。
    memory_calls: std::sync::Mutex<Vec<ChatRequest>>,
    /// v1.171 测试钩子：前 N 次标题调用返回 RateLimited（辅助调用重试断言）。
    title_fail_first: std::sync::Mutex<usize>,
}

impl MockProvider {
    pub fn new(name: &str, model: &str, script: Vec<ScriptedReply>) -> Self {
        Self {
            name: name.to_string(),
            model: model.to_string(),
            script: std::sync::Mutex::new(script),
            loop_last: true,
            calls: std::sync::Mutex::new(Vec::new()),
            title_calls: std::sync::Mutex::new(Vec::new()),
            memory_calls: std::sync::Mutex::new(Vec::new()),
            title_fail_first: std::sync::Mutex::new(0),
        }
    }

    /// v1.171 测试钩子：让前 n 次标题生成调用返回瞬时限流（验证辅助调用重试）。
    pub fn fail_first_title_calls(&self, n: usize) {
        *self.title_fail_first.lock().expect("title fail lock") = n;
    }

    pub fn calls(&self) -> Vec<ChatRequest> {
        self.calls.lock().expect("calls lock").clone()
    }

    pub fn title_calls(&self) -> Vec<ChatRequest> {
        self.title_calls.lock().expect("title calls lock").clone()
    }

    pub fn memory_calls(&self) -> Vec<ChatRequest> {
        self.memory_calls.lock().expect("memory calls lock").clone()
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
        // 标题生成请求（v1.58）：固定短标题回复，不消耗脚本队列、不进 calls，
        // 既有脚本化测试对调用次数与序列的断言不受自动标题影响。
        if req
            .messages
            .iter()
            .any(|m| m.content.contains(crate::TITLE_MARKER))
        {
            // v1.171 测试钩子：前 N 次瞬时限流（辅助调用重试路径）
            let mut fail_first = self.title_fail_first.lock().expect("title fail lock");
            if *fail_first > 0 {
                *fail_first -= 1;
                return Err(crate::ProviderError::RateLimited {
                    body: "mock 标题调用限流".into(),
                    retry_after: Some(std::time::Duration::from_millis(10)),
                });
            }
            drop(fail_first);
            self.title_calls
                .lock()
                .expect("title calls lock")
                .push(req.clone());
            return Ok(ChatResponse {
                content: "Mock 会话标题".into(),
                tool_calls: vec![],
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cached_input_tokens: 0,
                },
                model: self.model.clone(),
                finish_reason: Some("stop".into()),
                reasoning: String::new(),
            });
        }
        // 记忆提取请求（v1.104 §10.1）：固定记忆 JSON 回复，与标题同法——
        // 单独记录、不消耗脚本队列、不进 calls。
        if req
            .messages
            .iter()
            .any(|m| m.content.contains(crate::MEMORY_MARKER))
        {
            self.memory_calls
                .lock()
                .expect("memory calls lock")
                .push(req.clone());
            return Ok(ChatResponse {
                content: r#"{"memories":[{"kind":"preference","scope":"global","content":"Mock 记忆：回复用中文","importance":3}]}"#.into(),
                tool_calls: vec![],
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cached_input_tokens: 0,
                },
                model: self.model.clone(),
                finish_reason: Some("stop".into()),
                reasoning: String::new(),
            });
        }
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
                    // v1.129：模拟隐式缓存命中（50%），供缓存命中率链路测试断言
                    cached_input_tokens: input_tokens / 2,
                },
                model: self.model.clone(),
                finish_reason: Some("stop".into()),
                reasoning: String::new(),
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
                    cached_input_tokens: input_tokens / 2,
                },
                model: self.model.clone(),
                finish_reason: Some("tool_use".into()),
                reasoning: String::new(),
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
                    cached_input_tokens: input_tokens / 2,
                },
                model: self.model.clone(),
                finish_reason: Some("tool_use".into()),
                reasoning: String::new(),
            },
            ScriptedReply::Truncated(t) => ChatResponse {
                content: t,
                tool_calls: vec![],
                usage: Usage {
                    input_tokens,
                    output_tokens: 5,
                    cached_input_tokens: input_tokens / 2,
                },
                model: self.model.clone(),
                finish_reason: Some("length".into()),
                reasoning: String::new(),
            },
            ScriptedReply::Failure(msg) => {
                return Err(crate::ProviderError::Network(msg));
            }
            ScriptedReply::RateLimited { retry_after_ms } => {
                return Err(crate::ProviderError::RateLimited {
                    body: "mock 上游限流".into(),
                    retry_after: retry_after_ms.map(std::time::Duration::from_millis),
                });
            }
            ScriptedReply::NonTransient(msg) => {
                return Err(crate::ProviderError::Http {
                    status: 401,
                    body: msg,
                });
            }
            ScriptedReply::ContextOverflow => {
                return Err(crate::ProviderError::Http {
                    status: 400,
                    body: "This model's maximum context length is 8192 tokens. However, your messages resulted in 12290 tokens.".into(),
                });
            }
        };
        Ok(resp)
    }

    async fn chat_stream(&self, req: &ChatRequest) -> ProviderResult<ChatStream> {
        let final_response = self.chat(req).await?;
        let text = final_response.content.clone();
        let chars: Vec<char> = text.chars().collect();
        let mut events = Vec::new();
        for chunk in chars.chunks(8) {
            events.push(Ok(ChatStreamEvent::Delta(chunk.iter().collect())));
        }
        events.push(Ok(ChatStreamEvent::Final(final_response)));
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

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

    #[tokio::test]
    async fn text_stream_yields_unicode_safe_chunks_then_authoritative_final() {
        let mock = MockProvider::new(
            "mock",
            "mock-1",
            vec![ScriptedReply::Text("中文流式输出✅".into())],
        );
        let mut stream = mock
            .chat_stream(&ChatRequest::new("mock-1", vec![]))
            .await
            .unwrap();
        let mut deltas = String::new();
        let mut final_response = None;
        loop {
            match stream.next().await {
                Some(Ok(ChatStreamEvent::Delta(text))) => deltas.push_str(&text),
                Some(Ok(ChatStreamEvent::ReasoningDelta(_))) => {}
                Some(Ok(ChatStreamEvent::Final(resp))) => final_response = Some(resp),
                Some(Err(error)) => panic!("stream failed: {error}"),
                None => break,
            }
        }
        assert_eq!(deltas, "中文流式输出✅");
        assert_eq!(final_response.expect("final").content, deltas);
    }
}
