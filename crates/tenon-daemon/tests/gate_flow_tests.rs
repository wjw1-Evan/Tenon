//! v2.1 档位系统 daemon 级集成测试（design-v2.md §4.1，HTTP 全链）：
//! D 级 Hold → awaiting_confirm + pending_confirm 载荷 → POST /session/:id/confirm
//! 三键决议 → 回合继续；settings security.* 默认档注入；control set_approval /
//! set_exec_mode；confirm 路由契约（404 / 409 / 400）。

use std::sync::Arc;
use std::time::Duration;

use tenon_daemon::{serve, DaemonOptions};
use tenon_models::{MockProvider, ScriptedReply};

fn base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn client_with_token(token: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "X-Tenon-Token",
        reqwest::header::HeaderValue::from_str(token).unwrap(),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn start_daemon(script: Vec<ScriptedReply>) -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", script))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.worktrees_root = Some(dir.path().join("worktrees"));
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.policy_path = Some(dir.path().join("policy.toml"));
    options.updates_staging_dir = Some(dir.path().join("updates/staged"));
    options.laya_models_dir = Some(dir.path().join("models/laya"));
    let handle = serve(options).await.unwrap();
    (dir, handle.port, handle.token)
}

/// 注册项目 + 建会话（git 初始化可选——git_commit 需要真仓库）。
async fn create_session(
    client: &reqwest::Client,
    port: u16,
    dir: &tempfile::TempDir,
    git_init: bool,
) -> String {
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("code.txt"), "fn main() {}\n").unwrap();
    if git_init {
        for args in [
            vec!["init", "-q", "."],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&project)
                .args(&args)
                .output()
                .unwrap();
        }
    }
    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = registered["id"].as_str().unwrap().to_string();
    let created: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({
            "project_id": project_id,
            "provider": "mock",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    created["session_id"].as_str().unwrap().to_string()
}

async fn get_session(client: &reqwest::Client, port: u16, sid: &str) -> serde_json::Value {
    client
        .get(format!("{}/session/{sid}", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// 轮询至指定状态（默认档 on_irreversible 的 Hold 会卡到决议，不会超时）。
async fn wait_for_status(
    client: &reqwest::Client,
    port: u16,
    sid: &str,
    want: &str,
) -> serde_json::Value {
    for _ in 0..200 {
        let status = get_session(client, port, sid).await;
        if status["status"] == want {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("状态未到达 {want}");
}

#[tokio::test]
async fn d_level_hold_then_deny_resolves_and_completes_without_commit() {
    // v2.1 档位确认回路全链（design-v2.md §4.1）：默认档 on_irreversible——
    // D 级 git_commit 被Hold，GET /session 呈 awaiting_confirm + pending_confirm；
    // deny 决议后模型收到拒绝、回合继续；commit 不落盘。
    let (dir, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "git_commit".into(),
            args: serde_json::json!({"message": "should not land"}),
        },
        ScriptedReply::Text("用户拒绝了提交。".into()),
    ])
    .await;
    let client = client_with_token(&token);
    let sid = create_session(&client, port, &dir, true).await;

    let r = client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "提交代码"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);

    // Hold：状态 + 待确认载荷
    let held = wait_for_status(&client, port, &sid, "awaiting_confirm").await;
    assert_eq!(held["pending_confirm"]["tool"], "git_commit");
    assert_eq!(held["pending_confirm"]["level"], "d");
    assert!(
        held["pending_confirm"]["args"]
            .as_str()
            .unwrap()
            .contains("should not land"),
        "args 预览应含提交信息: {}",
        held["pending_confirm"]["args"]
    );
    // 默认档回显（settings 无覆盖 → Gate::default）
    assert_eq!(held["gate"]["approval"], "on_irreversible");
    assert_eq!(held["gate"]["exec_mode"], "workspace_write");

    // deny 决议 → 回合继续到 done
    let r: serde_json::Value = client
        .post(format!("{}/session/{sid}/confirm", base(port)))
        .json(&serde_json::json!({"decision": "deny"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["resolved"], true);
    let done = wait_for_status(&client, port, &sid, "done").await;
    assert!(done["outcome"].is_object(), "deny 后回合应正常完成");

    // commit 未落盘
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path().join("proj"))
        .args(["log", "--oneline"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).trim().is_empty());

    // trace：confirm_request + confirm_resolved(deny)
    let trace: serde_json::Value = client
        .get(format!("{}/session/{sid}/trace", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let events = trace["events"].as_array().unwrap();
    assert!(
        events.iter().any(|e| e["type"] == "confirm_request"),
        "应有确认请求事件"
    );
    let resolved = events
        .iter()
        .find(|e| e["type"] == "confirm_resolved")
        .expect("应有确认决议事件");
    assert_eq!(resolved["payload"]["decision"], "deny");
}

#[tokio::test]
async fn allow_session_memory_skips_second_hold_in_same_session() {
    // allow_session 决议：本会话同名工具后续不再 Hold（第二回合 git_commit
    // 直接执行——需先 apply_patch 制造真实变更，否则 nothing to commit）。
    let (dir, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "git_commit".into(),
            args: serde_json::json!({"message": "first commit"}),
        },
        ScriptedReply::Text("第一次完成".into()),
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({
                "file": "code2.txt", "range": null, "content": "second\n"
            }),
        },
        ScriptedReply::Tool {
            name: "git_commit".into(),
            args: serde_json::json!({"message": "second commit"}),
        },
        ScriptedReply::Text("第二次完成".into()),
    ])
    .await;
    let client = client_with_token(&token);
    let sid = create_session(&client, port, &dir, true).await;

    // 第一回合：git_commit → Hold → allow_session
    client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "提交一次"}))
        .send()
        .await
        .unwrap();
    wait_for_status(&client, port, &sid, "awaiting_confirm").await;
    let r = client
        .post(format!("{}/session/{sid}/confirm", base(port)))
        .json(&serde_json::json!({"decision": "allow_session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    wait_for_status(&client, port, &sid, "done").await;

    // 第二回合：apply_patch + git_commit——记忆生效不 Hold，直达 done
    client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "再改再提交"}))
        .send()
        .await
        .unwrap();
    let done = wait_for_status(&client, port, &sid, "done").await;
    assert!(done["outcome"].is_object());

    let trace: serde_json::Value = client
        .get(format!("{}/session/{sid}/trace", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let requests = trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "confirm_request")
        .count();
    assert_eq!(requests, 1, "allow_session 记忆后第二次 D 级不再 Hold");

    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path().join("proj"))
        .args(["log", "--oneline"])
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&log.stdout);
    assert!(log_text.contains("first commit"), "{log_text}");
    assert!(log_text.contains("second commit"), "{log_text}");
}

#[tokio::test]
async fn confirm_route_contracts() {
    let (dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // 未知会话 → 404
    let r = client
        .post(format!("{}/session/nonexistent/confirm", base(port)))
        .json(&serde_json::json!({"decision": "deny"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // 非法 decision → 400
    let sid = create_session(&client, port, &dir, false).await;
    let r = client
        .post(format!("{}/session/{sid}/confirm", base(port)))
        .json(&serde_json::json!({"decision": "maybe"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // 空闲会话无待确认 → 409
    let r = client
        .post(format!("{}/session/{sid}/confirm", base(port)))
        .json(&serde_json::json!({"decision": "allow_once"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
}

#[tokio::test]
async fn settings_security_keys_persist_and_apply_to_new_sessions() {
    let (dir, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "git_commit".into(),
            args: serde_json::json!({"message": "never-gear commit"}),
        },
        ScriptedReply::Text("完成".into()),
    ])
    .await;
    let client = client_with_token(&token);

    // PUT 合法档位 → 200；GET 回显；settings.json 落盘
    let r = client
        .put(format!("{}/settings", base(port)))
        .json(&serde_json::json!({
            "security": {"exec_mode": "read_only", "approval": "never"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "合法档位应可写");
    let settings: serde_json::Value = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["security"]["exec_mode"], "read_only");
    assert_eq!(settings["security"]["approval"], "never");
    let persisted = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    assert!(persisted.contains("\"never\""), "{persisted}");

    // 新会话注入默认档（gate 载荷回显）+ never 档 D 级直执不 Hold
    let sid = create_session(&client, port, &dir, true).await;
    let status = get_session(&client, port, &sid).await;
    assert_eq!(status["gate"]["approval"], "never");
    assert_eq!(status["gate"]["exec_mode"], "read_only");

    client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "提交代码"}))
        .send()
        .await
        .unwrap();
    // never 档不 Hold；read_only 派生会话只读 → D 级被只读开关拒绝（denied 错误
    // 事件，回合继续）——两个 v2.1 语义一道验证
    let done = wait_for_status(&client, port, &sid, "done").await;
    assert!(done["outcome"].is_object());
    let trace: serde_json::Value = client
        .get(format!("{}/session/{sid}/trace", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let events = trace["events"].as_array().unwrap();
    assert!(
        !events.iter().any(|e| e["type"] == "confirm_request"),
        "never 档不应产生确认请求"
    );
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "error" && e["payload"]["denied"] == "git_commit"),
        "read_only 档应拒绝 D 级: {}",
        serde_json::to_string(&events).unwrap_or_default()
    );

    // 非法档位值 → 400
    let r = client
        .put(format!("{}/settings", base(port)))
        .json(&serde_json::json!({"security": {"approval": "sometimes"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn control_gear_switch_updates_session_gate() {
    let (dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let sid = create_session(&client, port, &dir, false).await;

    // 非法 gear → 400
    let r = client
        .post(format!("{}/session/{sid}/control", base(port)))
        .json(&serde_json::json!({"action": "set_approval", "gear": "sometimes"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = client
        .post(format!("{}/session/{sid}/control", base(port)))
        .json(&serde_json::json!({"action": "set_exec_mode", "gear": "yolo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // 合法切换 → gate 载荷即时回显（空闲会话同样可切，下一任务生效）
    let r = client
        .post(format!("{}/session/{sid}/control", base(port)))
        .json(&serde_json::json!({"action": "set_approval", "gear": "always"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .post(format!("{}/session/{sid}/control", base(port)))
        .json(&serde_json::json!({"action": "set_exec_mode", "gear": "full_access"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let status = get_session(&client, port, &sid).await;
    assert_eq!(status["gate"]["approval"], "always");
    assert_eq!(status["gate"]["exec_mode"], "full_access");
}
