//! 流式协议回归：本地 SSE fake server，无外网依赖（§9.6 / §11）。

use futures::StreamExt;
use std::time::Duration;
use tenon_models::{
    AnthropicProvider, ChatMessage, ChatRequest, ChatStreamEvent, ModelProvider,
    OpenAiCompatProvider,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn serve_once(sse_body: &str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse_body}",
        sse_body.len()
    );
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0_u8; 16 * 1024];
        let _ = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut buf)).await;
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    format!("http://{addr}")
}

fn collect(
    events: Vec<tenon_models::ChatStreamEvent>,
) -> (String, Option<tenon_models::ChatResponse>) {
    let mut text = String::new();
    let mut final_response = None;
    for event in events {
        match event {
            ChatStreamEvent::Delta(delta) => text.push_str(&delta),
            ChatStreamEvent::Final(resp) => final_response = Some(resp),
        }
    }
    (text, final_response)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_sse_accumulates_text_and_tool_arguments() {
    let body = concat!(
        "data: {\"model\":\"m1\",\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"x\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\n",
        "data: [DONE]\n\n"
    );
    let base = serve_once(body).await;
    let provider = OpenAiCompatProvider::new("local", &base, "", Some("m1".into()));
    let mut req = ChatRequest::new("m1", vec![ChatMessage::user("read")]);
    req.tools = vec![tenon_models::ToolSpec {
        name: "read".into(),
        description: "read file".into(),
        parameters: serde_json::json!({"type":"object"}),
    }];
    let mut stream = provider.chat_stream(&req).await.unwrap();
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        events.push(item.unwrap());
    }
    let (text, resp) = collect(events);
    assert_eq!(text, "he");
    let resp = resp.unwrap();
    assert_eq!(resp.model, "m1");
    assert_eq!(resp.usage.input_tokens, 7);
    assert_eq!(resp.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(resp.tool_calls[0].id, "call_1");
    assert_eq!(resp.tool_calls[0].arguments["path"], "x");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_sse_accumulates_text_tool_and_usage() {
    let body = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"m2\",\"usage\":{\"input_tokens\":9}}}\n\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"y\\\"}\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":4}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    let base = serve_once(body).await;
    let provider = AnthropicProvider::new("local", &base, "", Some("m2".into()));
    let mut req = ChatRequest::new("m2", vec![ChatMessage::user("read")]);
    req.tools = vec![tenon_models::ToolSpec {
        name: "read".into(),
        description: "read file".into(),
        parameters: serde_json::json!({"type":"object"}),
    }];
    let mut stream = provider.chat_stream(&req).await.unwrap();
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        events.push(item.unwrap());
    }
    let (_text, resp) = collect(events);
    let resp = resp.unwrap();
    assert_eq!(resp.model, "m2");
    assert_eq!(resp.usage.input_tokens, 9);
    assert_eq!(resp.usage.output_tokens, 4);
    assert_eq!(resp.finish_reason.as_deref(), Some("tool_use"));
    assert_eq!(resp.tool_calls[0].arguments["path"], "y");
}
