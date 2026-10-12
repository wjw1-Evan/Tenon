//! ACP 客户端全链测试（design.md §13.3 v2.23）：mock agent（python）驱动
//! initialize → session/new → session/prompt（chunk 流 + tool_call 计数 +
//! stopReason 收尾）与 initialize 失败错误路径。python3 缺席环境跳过。

use std::path::PathBuf;
use tenon_acp::AcpAgent;

fn mock_agent() -> Option<AcpAgent> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/mock_acp_agent.py");
    if !script.exists() {
        return None;
    }
    AcpAgent::spawn("python3", &[script.to_string_lossy().into_owned()]).ok()
}

#[test]
fn full_chain_initialize_new_prompt_stream() {
    let Some(mut agent) = mock_agent() else {
        eprintln!("python3 不可用，跳过");
        return;
    };
    // initialize 握手：protocolVersion 1
    let init = agent.initialize().expect("initialize");
    assert_eq!(
        init.get("protocolVersion").and_then(|v| v.as_u64()),
        Some(1),
        "{init:?}"
    );
    // session/new
    let sid = agent
        .new_session(std::path::Path::new("/tmp"))
        .expect("session/new");
    assert_eq!(sid, "sess-1");
    // prompt：两条 chunk 拼接 + tool_call 计数 + end_turn
    let result = agent.prompt(&sid, "打个招呼").expect("prompt");
    assert_eq!(result.answer, "你好，任务已完成。");
    assert_eq!(result.stop_reason, "end_turn");
    let counts = result.updates;
    assert_eq!(
        counts.get("agent_message_chunk").and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(counts.get("tool_call").and_then(|v| v.as_u64()), Some(1));
}

#[test]
fn eof_after_shutdown_is_explicit() {
    let Some(mut agent) = mock_agent() else {
        eprintln!("python3 不可用，跳过");
        return;
    };
    agent.shutdown();
    // 进程回收后再 initialize：stdin 已关 → IO 错误（显式失败，非静默）
    let err = agent.initialize();
    assert!(
        matches!(
            err,
            Err(tenon_acp::AcpError::Io(_)) | Err(tenon_acp::AcpError::Protocol(_))
        ),
        "回收后应显式失败: {err:?}"
    );
}
