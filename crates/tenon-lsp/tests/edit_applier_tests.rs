//! 写回通道端到端测试（§8.5 v1.200/v1.202）：fake 服务器发起 workspace/applyEdit
//! → 宿主守卫放行 → 解析拼接 → EditApplier 落盘（复刻 daemon 接线语义）。

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{setup_host_with_applier, tmp_project, FakeServerBehavior};
use tenon_lsp::guard::GuardDecision;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 复刻 daemon 接线语义（state.rs v1.202）：读 → writedit 拼接 → 守卫语义写盘。
fn daemon_like_applier(
    root: PathBuf,
    edits: Vec<tenon_lsp::writedit::ServerFileEdit>,
) -> Result<usize, String> {
    let mut done = 0usize;
    for e in edits {
        let rel = e
            .path
            .strip_prefix(&root)
            .map_err(|_| format!("越界: {}", e.path.display()))?
            .to_string_lossy()
            .into_owned();
        let current = std::fs::read_to_string(&e.path).map_err(|er| format!("读取: {er}"))?;
        let new = tenon_lsp::writedit::apply_file_edits(&current, &e.edits)?;
        let target = root.join(&rel);
        std::fs::write(&target, new).map_err(|er| format!("写入: {er}"))?;
        done += 1;
    }
    Ok(done)
}

#[test]
fn apply_edit_roundtrip_writes_file_via_applier() {
    let dir = tmp_project();
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "hello\n").unwrap();
    let file_uri = format!("file://{}", file.to_string_lossy());
    let host = setup_host_with_applier(
        dir.path(),
        FakeServerBehavior {
            trigger_edit: Some(serde_json::json!({
                "changes": {file_uri: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}}, "newText": "X"}]}
            })),
            ..Default::default()
        },
        Arc::new(daemon_like_applier),
    );
    host.initialize(TIMEOUT).unwrap();
    host.request("test/trigger/applyEdit", serde_json::json!({}), TIMEOUT)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    // 守卫放行 + 执行器落盘：文件内容被真实更新
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "X\n");
    assert!(host
        .guard()
        .reports()
        .iter()
        .any(|r| r.method == "workspace/applyEdit" && r.decision == GuardDecision::Allowed));
    host.shutdown();
}

#[test]
fn apply_edit_applier_error_is_honest_no_write() {
    let dir = tmp_project();
    let file = dir.path().join("b.rs");
    std::fs::write(&file, "keep\n").unwrap();
    let file_uri = format!("file://{}", file.to_string_lossy());
    let host = setup_host_with_applier(
        dir.path(),
        FakeServerBehavior {
            trigger_edit: Some(serde_json::json!({
                "changes": {file_uri: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 4}}, "newText": "X"}]}
            })),
            ..Default::default()
        },
        // 执行器失败（如 daemon 写链 IO 错误）：错误如实上报，文件不得半写
        Arc::new(|_root: PathBuf, _edits| Err("注入失败".to_string())),
    );
    host.initialize(TIMEOUT).unwrap();
    host.request("test/trigger/applyEdit", serde_json::json!({}), TIMEOUT)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "keep\n",
        "失败不得落盘"
    );
    host.shutdown();
}
