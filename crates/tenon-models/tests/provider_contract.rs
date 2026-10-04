//! Provider 协议契约测试（设计方案 §11 / §18）。
//!
//! 用本地一次性 TCP 服务替代真实供应商，锁定三类关键边界：
//! 请求 wire format、鉴权 / 版本头，以及 HTTP 与 SSE 错误不得被吞掉。

use futures::StreamExt;
use tenon_models::{
    ChatMessage, ChatRequest, ChatStreamEvent, ModelProvider, ProviderError, Role, ToolCallReq,
    ToolSpec,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Debug)]
struct CapturedRequest {
    path: String,
    authorization: Option<String>,
    api_key: Option<String>,
    protocol_version: Option<String>,
    body: String,
}

async fn serve_http(
    status: &str,
    content_type: &str,
    body: &str,
) -> (String, tokio::task::JoinHandle<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = body.to_string();
    let status = status.to_string();
    let content_type = content_type.to_string();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut chunk = [0_u8; 16 * 1024];
        loop {
            let n = socket.read(&mut chunk).await.unwrap();
            raw.extend_from_slice(&chunk[..n]);
            let Some(header_end) = find_subslice(&raw, b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&raw[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if !name.trim().eq_ignore_ascii_case("content-length") {
                        return None;
                    }
                    value.trim().parse::<usize>().ok()
                })
                .unwrap_or(0);
            let body_start = header_end + 4;
            if raw.len() >= body_start + content_length {
                break;
            }
        }

        let request_line_end = find_subslice(&raw, b"\r\n").unwrap();
        let request_line = String::from_utf8_lossy(&raw[..request_line_end]);
        let path = request_line.split_whitespace().nth(1).unwrap().to_string();
        let headers_end = find_subslice(&raw, b"\r\n\r\n").unwrap();
        let headers = String::from_utf8_lossy(&raw[..headers_end]).to_string();
        let body_start = headers_end + 4;
        let request = CapturedRequest {
            path,
            authorization: header(&headers, "authorization").map(String::from),
            api_key: header(&headers, "x-api-key").map(String::from),
            protocol_version: header(&headers, "anthropic-version").map(String::from),
            body: String::from_utf8_lossy(&raw[body_start..]).to_string(),
        };

        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
        request
    });
    (format!("http://{addr}"), task)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn header<'a>(headers: &'a str, wanted: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(wanted)
            .then_some(value.trim())
    })
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "read".into(),
        description: "read a project file".into(),
        parameters: serde_json::json!({"type": "object"}),
    }
}

fn protocol_request() -> ChatRequest {
    let assistant = ChatMessage {
        role: Role::Assistant,
        content: "I will read it.".into(),
        tool_calls: vec![ToolCallReq {
            id: "call_1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        }],
        tool_call_id: None,
    };
    ChatRequest::new(
        "model-a",
        vec![
            ChatMessage::system("stay safe"),
            ChatMessage::user("fix it"),
            assistant,
            ChatMessage::tool_result("call_1", "file body"),
        ],
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_chat_maps_roles_tools_auth_and_reasoning_fallback() {
    let response = serde_json::json!({
        "model": "model-b",
        "choices": [{
            "message": {
                "content": "",
                "reasoning_content": "reasoning answer",
                "tool_calls": [{
                    "id": "call_9",
                    "function": {
                        "name": "read",
                        "arguments": "{\"path\":\"src/main.rs\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 11, "completion_tokens": 4}
    })
    .to_string();
    let (base, server) = serve_http("200 OK", "application/json", &response).await;
    let mut req = protocol_request();
    req.tools = vec![tool()];
    let provider = tenon_models::OpenAiCompatProvider::new("openai", &base, "secret", None);
    let resp = provider.chat(&req).await.unwrap();
    let request = server.await.unwrap();

    assert_eq!(request.path, "/chat/completions");
    assert_eq!(request.authorization.as_deref(), Some("Bearer secret"));
    let sent: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(sent["messages"][0]["role"], "system");
    assert_eq!(sent["messages"][3]["role"], "tool");
    assert_eq!(sent["messages"][3]["tool_call_id"], "call_1");
    assert_eq!(
        sent["messages"][2]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"src/main.rs\"}"
    );
    assert_eq!(sent["tools"][0]["function"]["name"], "read");
    assert_eq!(resp.content, "reasoning answer");
    assert_eq!(resp.tool_calls[0].id, "call_9");
    assert_eq!(resp.usage.output_tokens, 4);
    assert_eq!(resp.finish_reason.as_deref(), Some("tool_calls"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_chat_maps_messages_auth_and_blocks() {
    let response = serde_json::json!({
        "model": "claude-actual",
        "stop_reason": "tool_use",
        "content": [
            {"type": "text", "text": "need "},
            {"type": "text", "text": "a tool"},
            {"type": "tool_use", "id": "tool_9", "name": "read", "input": {"path": "x"}}
        ],
        "usage": {"input_tokens": 8, "output_tokens": 5}
    })
    .to_string();
    let (base, server) = serve_http("200 OK", "application/json", &response).await;
    let mut req = protocol_request();
    req.tools = vec![tool()];
    let provider = tenon_models::AnthropicProvider::new("anthropic", &base, "secret", None);
    let resp = provider.chat(&req).await.unwrap();
    let request = server.await.unwrap();

    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.api_key.as_deref(), Some("secret"));
    assert_eq!(request.protocol_version.as_deref(), Some("2023-06-01"));
    let sent: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(sent["system"], "stay safe");
    assert_eq!(sent["messages"][0]["content"][0]["type"], "text");
    assert_eq!(sent["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(sent["messages"][2]["content"][0]["tool_use_id"], "call_1");
    assert_eq!(sent["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(resp.content, "need a tool");
    assert_eq!(resp.tool_calls[0].id, "tool_9");
    assert_eq!(resp.usage.input_tokens, 8);
    assert_eq!(resp.finish_reason.as_deref(), Some("tool_use"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_http_errors_map_to_status_and_body() {
    for provider in ["openai", "anthropic"] {
        let (base, server) =
            serve_http("429 Too Many Requests", "application/json", "rate limited").await;
        let req = ChatRequest::new(
            if provider == "openai" {
                "model-a"
            } else {
                "model-b"
            },
            vec![ChatMessage::user("hi")],
        );
        let result = if provider == "openai" {
            tenon_models::OpenAiCompatProvider::new(provider, &base, "", None)
                .chat(&req)
                .await
        } else {
            tenon_models::AnthropicProvider::new(provider, &base, "", None)
                .chat(&req)
                .await
        };
        server.await.unwrap();

        let Err(ProviderError::Http { status, body }) = result else {
            panic!("{provider} HTTP 错误必须映射为 ProviderError::Http");
        };
        assert_eq!(status, 429);
        assert_eq!(body, "rate limited");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_stream_rejects_bad_sse_json_and_missing_done() {
    let (base, _) = serve_http("200 OK", "text/event-stream", "data: {not-json}\n\n").await;
    let req = ChatRequest::new("model-a", vec![ChatMessage::user("hi")]);
    let mut stream = tenon_models::OpenAiCompatProvider::new("p", &base, "", None)
        .chat_stream(&req)
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Err(ProviderError::Parse(_)))
    ));

    let (base, _) = serve_http(
        "200 OK",
        "text/event-stream",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
    )
    .await;
    let mut stream = tenon_models::OpenAiCompatProvider::new("p", &base, "", None)
        .chat_stream(&req)
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Ok(ChatStreamEvent::Delta(text))) if text == "partial"
    ));
    assert!(matches!(
        stream.next().await,
        Some(Err(ProviderError::Network(message))) if message.contains("[DONE]")
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_stream_rejects_bad_sse_json_and_missing_stop() {
    let (base, _) = serve_http(
        "200 OK",
        "text/event-stream",
        "data: {\"type\":\"message_start\"\n\n",
    )
    .await;
    let req = ChatRequest::new("model-b", vec![ChatMessage::user("hi")]);
    let mut stream = tenon_models::AnthropicProvider::new("p", &base, "", None)
        .chat_stream(&req)
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Err(ProviderError::Parse(_)))
    ));

    let (base, _) = serve_http(
        "200 OK",
        "text/event-stream",
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"model-c\"}}\n\n",
    )
    .await;
    let mut stream = tenon_models::AnthropicProvider::new("p", &base, "", None)
        .chat_stream(&req)
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Err(ProviderError::Network(message))) if message.contains("message_stop")
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authoritative_stream_final_rejects_corrupt_tool_arguments() {
    let openai_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"read\",\"arguments\":\"{bad\"}}]}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let (base, _) = serve_http("200 OK", "text/event-stream", openai_body).await;
    let req = ChatRequest::new("model-a", vec![ChatMessage::user("read")]);
    let mut stream = tenon_models::OpenAiCompatProvider::new("p", &base, "", None)
        .chat_stream(&req)
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Err(ProviderError::Parse(message))) if message.contains("tool call")
    ));

    let anthropic_body = concat!(
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{bad\"}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    let (base, _) = serve_http("200 OK", "text/event-stream", anthropic_body).await;
    let req = ChatRequest::new("model-b", vec![ChatMessage::user("read")]);
    let mut stream = tenon_models::AnthropicProvider::new("p", &base, "", None)
        .chat_stream(&req)
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Err(ProviderError::Parse(message))) if message.contains("tool block")
    ));
}
