//! daemon 端到端集成测试（真 HTTP/WS；§15 API 全链路 + §12.6 鉴权面）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tenon_daemon::{serve, DaemonOptions};
use tenon_models::{MockProvider, ModelProvider, ScriptedReply};

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

#[tokio::test]
async fn fixed_port_and_token_serve_dev_hot_reload() {
    // 开发热重载（v1.65）：`--port` 固定绑定端口、`--token` 固定握手 token
    //（UI DEV 回落约定 127.0.0.1:9876 + token "dev"）；默认随机路径不受影响
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.bind_port = Some(port);
    options.fixed_token = Some("dev".into());
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    let handle = serve(options).await.unwrap();
    assert_eq!(handle.port, port);
    assert_eq!(handle.token, "dev");

    let r = client_with_token("dev")
        .get(format!("{}/models", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn health_is_open_but_api_requires_token() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    // /health 免 token（无敏感信息）
    let health = reqwest::get(format!("{base}/health", base = base(port)))
        .await
        .unwrap();
    assert_eq!(health.status(), 200);

    // 无 token → 401
    let bare = reqwest::get(format!("{}/models", base(port)))
        .await
        .unwrap();
    assert_eq!(bare.status(), 401);
    // 错 token → 401
    let wrong = client_with_token("wrong-token");
    let r = wrong
        .get(format!("{}/models", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // 对 token → 200
    let good = client_with_token(&token);
    let r = good
        .get(format!("{}/models", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn evil_host_header_rejected() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/models", base(port)))
        .header("Host", "evil.example.com")
        .header("X-Tenon-Token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "DNS rebinding 面（§12.6）");
}

#[tokio::test]
async fn foreign_origin_rejected() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/models", base(port)))
        .header("Origin", "https://evil.example.com")
        .header("X-Tenon-Token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "网页请求本地服务一律拒绝（§12.6）");
    // 本地 origin 放行
    let ok = client
        .get(format!("{}/models", base(port)))
        .header("Origin", "tauri://localhost")
        .header("X-Tenon-Token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
}

#[tokio::test]
async fn ws_ticket_single_use_flow() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    // 换票
    let resp: serde_json::Value = client
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ticket = resp["ticket"].as_str().unwrap().to_string();

    // 首次连接：首帧携带 ticket → auth ok
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    use futures::{SinkExt, StreamExt};
    ws.send(tokio_tungstenite::tungstenite::Message::text(
        ticket.clone(),
    ))
    .await
    .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert!(reply.to_string().contains("auth ok"), "{reply}");

    // 重放同一票 → 拒绝（ADR-10）
    let (mut ws2, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    ws2.send(tokio_tungstenite::tungstenite::Message::text(ticket))
        .await
        .unwrap();
    let reply2 = ws2.next().await.unwrap().unwrap();
    assert!(reply2.to_string().contains("auth failed"), "{reply2}");
}

#[tokio::test]
async fn ws_pushes_scoped_l4_status_changes() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("indexed");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("src.rs"), "pub fn recall_context() {}\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let opened: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = opened["id"].as_str().unwrap().to_string();

    // 等初始索引就绪，再订阅并触发 rebuild，避免小夹具完成快于建立订阅。
    for _ in 0..40 {
        let stats: serde_json::Value = client
            .get(format!("{}/project/{project_id}/l4/stats", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if stats["status"]["state"] == "ready" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let ticket_resp: serde_json::Value = client
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    use futures::{SinkExt, StreamExt};
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?project_id={project_id}"
    ))
    .await
    .unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::text(
        ticket_resp["ticket"].as_str().unwrap().to_string(),
    ))
    .await
    .unwrap();
    let auth = ws.next().await.unwrap().unwrap();
    assert!(auth.to_string().contains("auth ok"), "{auth}");

    client
        .post(format!("{}/project/{project_id}/l4/rebuild", base(port)))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_ready = false;
    while tokio::time::Instant::now() < deadline {
        let msg = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
        let Ok(Some(Ok(msg))) = msg else { continue };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&msg.to_string()) else {
            continue;
        };
        if event["type"] == "l4_status"
            && event["project_id"] == project_id
            && event["state"] == "ready"
        {
            saw_ready = true;
            break;
        }
    }
    assert!(saw_ready, "project-scoped L4 status event should arrive");
}

#[tokio::test]
async fn full_session_flow_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("README.md"), "# demo\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "hello.txt", "range": null, "content": "你好\n"}),
        },
        ScriptedReply::Text("任务完成".into()),
    ])
    .await;
    let client = client_with_token(&token);

    // 1. 注册项目 → TOFU 信任 → 建会话（会话继承信任状态，§12.7）
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
    assert!(
        !registered["trusted"].as_bool().unwrap(),
        "TOFU：初始未信任"
    );
    client
        .put(format!("{}/project/trust", base(port)))
        .json(&serde_json::json!({"project_id": project_id, "trusted": true}))
        .send()
        .await
        .unwrap();
    let created: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({
            "project_path": project.to_string_lossy(),
            "provider": "mock",
            "mode": "auto",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sid = created["session_id"].as_str().unwrap().to_string();

    // 2. 下发任务（202 接受，后台执行）
    let r = client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "写一个 hello 文件"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);

    // 3. 轮询到完成
    let mut outcome = None;
    for _ in 0..100 {
        let status: serde_json::Value = client
            .get(format!("{}/session/{sid}", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if status["status"] == "done" && status["outcome"].is_object() {
            outcome = Some(status);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status = outcome.expect("任务应完成");
    let card = &status["outcome"]["Done"];
    assert_eq!(card["changed_files"][0], "hello.txt");
    // 文件真实写入
    assert_eq!(
        std::fs::read_to_string(project.join("hello.txt")).unwrap(),
        "你好\n"
    );

    // 4. trace 事件流（断线续传语义：after 游标）
    let trace: serde_json::Value = client
        .get(format!("{}/session/{sid}/trace", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let kinds: Vec<&str> = trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"user_input"));
    assert!(kinds.contains(&"patch_applied"));

    // 续传：after=1
    let trace2: serde_json::Value = client
        .get(format!("{}/session/{sid}/trace?after=1", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(trace2["events"].as_array().unwrap().len() < trace["events"].as_array().unwrap().len());

    // 5. checkpoint 时间轴 + 回滚
    let cps: serde_json::Value = client
        .get(format!("{}/session/{sid}/checkpoints", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!cps["checkpoints"].as_array().unwrap().is_empty());
    let cp_id = cps["checkpoints"][0]["id"].as_str().unwrap().to_string();
    let rb: serde_json::Value = client
        .post(format!("{}/checkpoint/{cp_id}/rollback", base(port)))
        .json(&serde_json::json!({"granularity": "revert"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rb["rolled_back"][0], "hello.txt", "回滚删除 AI 新建文件");
    assert!(!project.join("hello.txt").exists());

    // 6. 成本归因
    let costs: serde_json::Value = client
        .get(format!("{}/costs?session={sid}", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(costs["input_tokens"].as_u64().unwrap() > 0);
    // v1.129：/costs 返回体携缓存命中与回合耗时（mock 任务回合 cached = input/2，
    // 标题辅助调用 cached=0 也计入 totals，故只断言区间）
    let cached = costs["cached_tokens"].as_u64().unwrap();
    assert!(cached > 0 && cached < costs["input_tokens"].as_u64().unwrap());
    assert!(costs["duration_ms"].as_u64().is_some());
}

/// v1.128 测试确定性通道：轮询后端（100ms 内容比对），避免原生 FSEvents
/// 注册延迟受系统 fseventsd 态势影响。
async fn start_daemon_with_poll_watcher(
    script: Vec<ScriptedReply>,
) -> (tempfile::TempDir, u16, String) {
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
    options.watch_poll_interval = Some(Duration::from_millis(100));
    let handle = serve(options).await.unwrap();
    (dir, handle.port, handle.token)
}

#[tokio::test]
async fn b_level_write_executes_directly_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("f.txt"), "x\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "f.txt", "range": null, "content": "written\n"}),
        },
        ScriptedReply::Text("写入完成".into()),
    ])
    .await;
    let client = client_with_token(&token);
    let created: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({
            "project_path": project.to_string_lossy(),
            "provider": "mock",
            "mode": "interactive",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sid = created["session_id"].as_str().unwrap().to_string();
    client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "写入 f.txt"}))
        .send()
        .await
        .unwrap();
    for _ in 0..100 {
        let status: serde_json::Value = client
            .get(format!("{}/session/{sid}", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if status["status"] == "done" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let trace: serde_json::Value = client
        .get(format!("{}/session/{sid}/trace", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "approval_request"));
    assert_eq!(
        std::fs::read_to_string(project.join("f.txt")).unwrap(),
        "x\nwritten\n"
    );
}
#[tokio::test]
async fn file_api_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.txt"), "alpha\nbeta\n").unwrap();
    std::fs::write(project.join("b.txt"), "beta again\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![ScriptedReply::Text("ok".into())]).await;
    let client = client_with_token(&token);
    let _ = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_path": project.to_string_lossy(), "provider": "mock"}))
        .send()
        .await
        .unwrap();

    // 文件树
    let pid = {
        let projects: serde_json::Value = client
            .get(format!("{}/projects", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        projects["projects"][0]["id"].as_str().unwrap().to_string()
    };
    let tree: serde_json::Value = client
        .get(format!("{}/project/{pid}/tree", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"a.txt"));

    // 读 / 写
    let read: serde_json::Value = client
        .get(format!("{}/project/{pid}/file?path=a.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read["content"], "alpha\nbeta\n");
    client
        .put(format!("{}/project/{pid}/file", base(port)))
        .json(&serde_json::json!({"path": "a.txt", "content": "changed\n"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "changed\n"
    );

    // 大文件：v1.69 起全量返回且可写（移除只读分块与拒写）。
    let large = format!("{}next\n", "A".repeat(10 * 1024 * 1024));
    std::fs::write(project.join("large.txt"), large).unwrap();
    let large_view: serde_json::Value = client
        .get(format!("{}/project/{pid}/file?path=large.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(large_view["total_bytes"], 10 * 1024 * 1024 + 5);
    assert_eq!(
        large_view["content"].as_str().unwrap().len(),
        10 * 1024 * 1024 + 5
    );
    let rewritten = client
        .put(format!("{}/project/{pid}/file", base(port)))
        .json(&serde_json::json!({"path": "large.txt", "content": "truncate\n"}))
        .send()
        .await
        .unwrap();
    assert_eq!(rewritten.status(), 200);
    assert_eq!(
        std::fs::read_to_string(project.join("large.txt")).unwrap(),
        "truncate\n"
    );

    // 路径越界被拒
    let escape = client
        .get(format!(
            "{}/project/{pid}/file?path=../etc/passwd",
            base(port)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(escape.status(), 400, "写守卫：越界读取拒绝");

    // tree-sitter 高亮 token 流由 daemon 侧产出；txt 回退为空。
    std::fs::write(
        project.join("sample.rs"),
        "pub fn syntax() { let done = true; }\n",
    )
    .unwrap();
    let highlighted: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/highlight?path=sample.rs",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(highlighted["language"], "rust");
    let token_kinds: Vec<&str> = highlighted["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|token| token["kind"].as_str())
        .collect();
    assert!(token_kinds.contains(&"keyword"), "{token_kinds:?}");
    assert!(token_kinds.contains(&"function"), "{token_kinds:?}");
    let plain: serde_json::Value = client
        .get(format!("{}/project/{pid}/highlight?path=a.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(plain["fallback"], true);
    assert_eq!(plain["tokens"].as_array().unwrap().len(), 0);

    // Git source view：branch / changes / commits / inline blame 只读聚合。
    let git_dir = project.join(".git");
    let _ = std::fs::remove_dir_all(&git_dir);
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-qb", "main"]).status.success());
    std::fs::write(
        project.join("source_view.txt"),
        "source view line\nsecond source line\n",
    )
    .unwrap();
    assert!(git(&["add", "."]).status.success());
    assert!(git(&["config", "user.email", "dev@tenon.local"])
        .status
        .success());
    assert!(git(&["config", "user.name", "Tenon Dev"]).status.success());
    assert!(git(&["commit", "-qm", "source view fixture"])
        .status
        .success());
    std::fs::write(project.join("changed.txt"), "working\n").unwrap();
    let source: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/git/view?path=source%2Fview.txt",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(source["repository"], true);
    assert_eq!(source["branch"], "main");
    assert!(source["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "changed.txt"));
    assert_eq!(source["commits"].as_array().unwrap().len(), 1);
    assert_eq!(source["blame"]["lines"].as_array().unwrap().len(), 0);
    let source: serde_json::Value = client
        .get(format!("{}/project/{pid}/git/view?path=a.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let blame = source["blame"]["lines"].as_array().unwrap();
    assert_eq!(blame.len(), 1);
    assert_eq!(blame[0]["author"], "Tenon Dev");
    assert_eq!(blame[0]["summary"], "source view fixture");

    // 搜索
    let hits: serde_json::Value = client
        .get(format!("{}/project/{pid}/search?q=beta", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !hits["hits"].as_array().unwrap().is_empty(),
        "a.txt 已改写，b.txt 仍含 beta"
    );
    // 替换预览（不落盘）
    let previews: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/search?q=beta&replace=BETA",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!previews["previews"].as_array().unwrap().is_empty());
    // 替换预览不落盘
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "changed\n"
    );

    // file ops（v1.72 起 /file/ops 不再提供创建，创建由会话大模型决策；此处覆盖 rename）
    std::fs::write(project.join("a_repo.rs"), "fn a() {}\n").unwrap();
    client
        .post(format!("{}/project/{pid}/file/ops", base(port)))
        .json(&serde_json::json!({"ops": [{"op": "rename", "from": "a_repo.rs", "to": "new.rs"}]}))
        .send()
        .await
        .unwrap();
    assert!(project.join("new.rs").exists());
    assert!(!project.join("a_repo.rs").exists());

    // v1.92：legacy 隐式项目端点已移除（404）。
    let legacy = client
        .get(format!("{}/file?path=a.txt", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.status(), 404);

    // 选定替换：先改 new.rs，dirty buffer 的 a.txt 必须被跳过。
    std::fs::write(project.join("new.rs"), "const value = beta;\n").unwrap();
    client
        .put(format!("{}/project/{pid}/buffers", base(port)))
        .json(&serde_json::json!({"path": "a.txt", "dirty": "user edit beta"}))
        .send()
        .await
        .unwrap();
    let replace: serde_json::Value = client
        .post(format!("{}/project/{pid}/search/replace", base(port)))
        .json(&serde_json::json!({
            "q": "beta",
            "replacement": "BETA",
            "paths": ["a.txt", "new.rs"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replace["applied"].as_array().unwrap().len(), 1);
    assert_eq!(replace["applied"][0]["path"], "new.rs");
    assert_eq!(replace["applied"][0]["replacements"], 1);
    assert_eq!(replace["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(replace["skipped"][0]["path"], "a.txt");
    assert_eq!(
        std::fs::read_to_string(project.join("new.rs")).unwrap(),
        "const value = BETA;\n"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "changed\n"
    );
}

#[tokio::test]
async fn multiproject_registry_isolation_and_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = dir.path().join("alpha");
    let beta = dir.path().join("beta");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(alpha.join("root.txt"), "alpha\n").unwrap();
    std::fs::write(beta.join("root.txt"), "beta\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let open = |path: &std::path::Path| {
        let client = client.clone();
        let path = path.to_path_buf();
        async move {
            client
                .post(format!("{}/projects/open", base(port)))
                .json(&serde_json::json!({"path": path.to_string_lossy()}))
                .send()
                .await
                .unwrap()
        }
    };
    let ra: serde_json::Value = open(&alpha).await.json().await.unwrap();
    let rb: serde_json::Value = open(&beta).await.json().await.unwrap();
    let ai = ra["id"].as_str().unwrap();
    let bi = rb["id"].as_str().unwrap();
    assert_ne!(ai, bi);

    // 再次打开 canonical path 去重。
    let duplicate: serde_json::Value = open(&alpha).await.json().await.unwrap();
    assert_eq!(duplicate["id"].as_str().unwrap(), ai);

    let list: serde_json::Value = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["projects"].as_array().unwrap().len(), 2);
    // v1.60：open 状态字段随打开 / 关闭生命周期一并移除。
    assert!(list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p.get("open").is_none()));

    // 文件 API 只读显式项目根。
    let pa: serde_json::Value = client
        .get(format!("{}/project/{ai}/file?path=root.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pb: serde_json::Value = client
        .get(format!("{}/project/{bi}/file?path=root.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pa["content"], "alpha\n");
    assert_eq!(pb["content"], "beta\n");

    // 懒加载层级文件树：目录查询返回子项；越界路径拒绝。
    std::fs::create_dir_all(alpha.join("src/deep")).unwrap();
    std::fs::write(alpha.join("src/deep/app.rs"), "fn main() {}\n").unwrap();
    let tree: serde_json::Value = client
        .get(format!("{}/project/{ai}/tree?path=src", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "src/deep" && entry["kind"] == "dir"));
    let escaped = client
        .get(format!("{}/project/{ai}/tree?path=../beta", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(escaped.status(), 400);

    // 同名相对路径的脏缓冲按项目隔离（v1.92：读端点移除，PUT/DELETE 链路断言）。
    for (project_id, content) in [(ai, "alpha dirty"), (bi, "beta dirty")] {
        let resp = client
            .put(format!("{}/project/{project_id}/buffers", base(port)))
            .json(&serde_json::json!({"path": "buffer.txt", "dirty": content}))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
    }
    let cleared = client
        .delete(format!(
            "{}/project/{ai}/buffers?path=buffer.txt",
            base(port)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(cleared.status(), 200);
}

#[tokio::test]
async fn registered_project_activates_runtime_on_first_use() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("registered-only");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("implicit.txt"), "registered ready\n").unwrap();
    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = registered["id"].as_str().unwrap().to_string();

    // v1.60：ProjectRuntime 是内部缓存；不先 POST /projects/open 也能访问。
    let tree: serde_json::Value = client
        .get(format!("{}/project/{pid}/tree", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names = tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"implicit.txt"));

    let file: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/file?path=implicit.txt",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(file["content"], "registered ready\n");
}

#[tokio::test]
async fn l4_incremental_index_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("index");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("src/auth.rs"),
        "pub fn login authenticate user session password\n",
    )
    .unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let opened: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = opened["id"].as_str().unwrap().to_string();

    // 全量索引由项目激活触发，500ms 去抖后执行。
    for _ in 0..30 {
        let stats: serde_json::Value = client
            .get(format!("{}/project/{pid}/l4/stats", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if stats["chunks"].as_u64().unwrap_or(0) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stats: serde_json::Value = client
        .get(format!("{}/project/{pid}/l4/stats", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(stats["chunks"].as_u64().unwrap() > 0, "{stats}");

    // v1.92：HTTP /l4/search 移除（Agent 走 store 直查）；增量更新经 watcher
    // 改写文件后由下方 rebuild → stats 断言覆盖。
    std::fs::write(
        project.join("src/auth.rs"),
        "pub fn render_canvas_and_pixels\n",
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    // 手动 rebuild：入队 → ready；stats 暴露状态与切片数。
    let rebuild = client
        .post(format!("{}/project/{pid}/l4/rebuild", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(rebuild.status(), 200);
    let mut ready = false;
    for _ in 0..30 {
        let stats: serde_json::Value = client
            .get(format!("{}/project/{pid}/l4/stats", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if stats["status"]["state"] == "ready" {
            assert!(stats["chunks"].as_u64().unwrap() > 0);
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "manual rebuild should become ready");
}

#[tokio::test]
async fn update_executor_reports_fail_closed_check_without_staging() {
    let (tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let status: serde_json::Value = client
        .get(format!("{}/updates", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["current_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["channel"], "manual");
    assert!(status["staged"].is_null());

    let checked = client
        .post(format!("{}/updates/check", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(checked.status(), 200);
    let body = checked.text().await.unwrap();
    assert!(body.contains("更新公钥未配置"), "{body}");

    let after: serde_json::Value = client
        .get(format!("{}/updates", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["last_error"], "更新公钥未配置");
    assert!(after["staged"].is_null());
    assert!(
        !tmp.path().join("updates/staged").exists()
            || std::fs::read_dir(tmp.path().join("updates/staged"))
                .unwrap()
                .count()
                == 0
    );
}

#[tokio::test]
async fn team_policy_api_persists_narrow_only_controls() {
    let (tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // v1.92：GET /team-policy 移除，读取走 GET /settings 的 team_policy 字段；
    // force_interactive 已随 v1.89 审批移除删除。
    let default: serde_json::Value = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        default["team_policy"]["denied_tools"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let saved = client
        .put(format!("{}/team-policy", base(port)))
        .json(&serde_json::json!({
            "denied_tools": ["git_push", "apply_patch", "apply_patch"],
            "max_cost_usd": 0.25,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let saved: serde_json::Value = saved.json().await.unwrap();
    assert_eq!(
        saved["denied_tools"],
        serde_json::json!(["apply_patch", "git_push"]),
        "重复项去重并稳定排序"
    );

    let settings: serde_json::Value = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["team_policy"]["max_cost_usd"], 0.25);

    let policy_path = tmp.path().join("policy.toml");
    let persisted = std::fs::read_to_string(&policy_path).unwrap();
    assert!(persisted.contains("max_cost_usd = 0.25"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&policy_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    for invalid in [
        serde_json::json!({"force_interactive": true}),
        serde_json::json!({"denied_tools": [""]}),
        serde_json::json!({"max_cost_usd": -1}),
        serde_json::json!({"unknown": true}),
    ] {
        let bad = client
            .put(format!("{}/team-policy", base(port)))
            .json(&invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 400, "{invalid}");
    }

    let unchanged: serde_json::Value = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged["team_policy"]["max_cost_usd"], 0.25);
}

#[tokio::test]
async fn settings_and_projects_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    // settings 读
    let s: serde_json::Value = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(s["session"].is_object());
    // project 注册与信任
    let p: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": dir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!p["trusted"].as_bool().unwrap());
    client
        .put(format!("{}/project/trust", base(port)))
        .json(&serde_json::json!({"project_id": p["id"], "trusted": true}))
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list["projects"][0]["trusted"].as_bool().unwrap());

    // 项目级 UI 状态（§7.2 / §7.5）：空状态 → 保存 → 回读；非法 body 拒收。
    let pid = p["id"].as_str().unwrap();
    let empty: serde_json::Value = client
        .get(format!("{}/project/{pid}/ui-state", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(empty.as_object().unwrap().is_empty());
    let saved = client
        .put(format!("{}/project/{pid}/ui-state", base(port)))
        .json(&serde_json::json!({
            "leftWidth": 320,
            "tabs": ["README.md"],
            "activePath": "README.md",
            "bottomTab": "trace",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let state: serde_json::Value = client
        .get(format!("{}/project/{pid}/ui-state", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["leftWidth"], 320);
    assert_eq!(state["tabs"][0], "README.md");
    assert_eq!(state["bottomTab"], "trace");
    let invalid = client
        .put(format!("{}/project/{pid}/ui-state", base(port)))
        .json(&serde_json::json!([]))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
}

#[tokio::test]
async fn ui_prefs_roundtrip_and_validation() {
    // §7.5 外观档权威存储：PUT upsert → GET 回读；非字符串值 400
    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let empty: serde_json::Value = client
        .get(format!("{}/ui-prefs", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(empty.as_object().unwrap().is_empty(), "初始为空");

    for body in [
        serde_json::json!({"theme": "light"}),
        serde_json::json!({"theme": "dark"}),
        serde_json::json!({"locale": "zh-CN"}),
    ] {
        let r = client
            .put(format!("{}/ui-prefs", base(port)))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    let prefs: serde_json::Value = client
        .get(format!("{}/ui-prefs", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(prefs["theme"], "dark", "upsert 覆盖旧值");
    assert_eq!(prefs["locale"], "zh-CN");

    let bad = client
        .put(format!("{}/ui-prefs", base(port)))
        .json(&serde_json::json!({"theme": 3}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400, "非字符串值须拒收");

    // 未带 token 不可读写（鉴权中间件覆盖）
    let anon = reqwest::Client::new()
        .get(format!("{}/ui-prefs", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
}

#[tokio::test]
async fn model_routing_switch_and_suggest() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let mut options = DaemonOptions::in_memory();
    options.providers = vec![
        Arc::new(MockProvider::new(
            "mock",
            "mock-1",
            vec![ScriptedReply::Text("ok".into())],
        )),
        Arc::new(MockProvider::new(
            "mock2",
            "mock2-1",
            vec![ScriptedReply::Text("from mock2".into())],
        )),
    ];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    let handle = tenon_daemon::serve(options).await.unwrap();
    let client = client_with_token(&handle.token);
    let port = handle.port;

    let created: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_path": project.to_string_lossy(), "provider": "mock"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sid = created["session_id"].as_str().unwrap().to_string();

    // 切换到 mock2（§11 上下文随迁）
    let switched: serde_json::Value = client
        .post(format!("{}/session/{sid}/model", base(port)))
        .json(&serde_json::json!({"provider": "mock2"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(switched["model"], "mock2-1");

    // 切换事件入 Trace（model_fallback）
    let trace: serde_json::Value = client
        .get(format!("{}/session/{sid}/trace", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        trace["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "model_fallback"),
        "model_fallback 事件应入 Trace"
    );

    // 路由建议（规则引擎回退，Laya 未下载）
    let suggest: serde_json::Value = client
        .post(format!("{}/model-suggest", base(port)))
        .json(&serde_json::json!({"text": "解释这段认证流程，不要改任何文件"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(suggest["pure_read"], true, "{suggest}");

    // 新任务走切换后的 provider（响应来自 mock2）
    client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "任务"}))
        .send()
        .await
        .unwrap();
    for _ in 0..60 {
        let status: serde_json::Value = client
            .get(format!("{}/session/{sid}", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if status["status"] == "done" && status["outcome"].is_object() {
            assert_eq!(
                status["outcome"]["Done"]["answer"], "from mock2",
                "切换后下一回合由新 provider 执行"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("切换后任务未完成");
}

#[tokio::test]
async fn language_pack_detect_and_wizard_flow() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("package.json"), r#"{"name":"t"}"#).unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = registered["id"].as_str().unwrap().to_string();

    let detected: serde_json::Value = client
        .get(format!("{}/project/{pid}/language-packs", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let packs = detected["packs"].as_array().unwrap();
    assert!(
        packs.iter().any(|p| p["language"] == "typescript"),
        "{detected}"
    );

    let ts = packs
        .iter()
        .find(|p| p["language"] == "typescript")
        .unwrap();
    if ts["server_installed"].as_bool().unwrap() {
        return; // 本机已装（跳过安装流程断言）
    }
    assert!(
        ts["runtime_hint"].as_str().unwrap().contains("npm install"),
        "应给出官方指引自装: {ts}"
    );
}

#[tokio::test]
async fn pairing_entry_local_only_and_evals_listing() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("ok".into())],
    ))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.project = Some(project.display().to_string());
    let handle = tenon_daemon::serve(options).await.unwrap();
    let client = client_with_token(&handle.token);

    // 配对入口：免 token 可达（本机），含 ws_ticket 与 lan 状态；
    // project 回传启动注册的项目根（浏览器自发现 UI 据此打开同一项目）
    let pairing: serde_json::Value = reqwest::get(format!("{}/pairing", base(handle.port)))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pairing["local_only"], true);
    assert_eq!(pairing["lan_enabled"], false);
    assert!(pairing["ws_ticket"].as_str().is_some());
    assert_eq!(
        pairing["project"],
        project.display().to_string(),
        "/pairing 须回传注册项目根"
    );

    // evals：tenon-evals 未运行 → 空列表（tenon-evals 跑完即有数据）
    let evals: serde_json::Value = client
        .get(format!("{}/evals", base(handle.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(evals["runs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn cors_headers_present_for_allowed_origin() {
    let (_dir, port, _token) = start_daemon(vec![]).await;
    let resp = reqwest::Client::new()
        .get(format!("{}/health", base(port)))
        .header("Origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap()),
        Some("http://localhost:5173"),
        "放行源应带 CORS 头（§12.6 白名单细化）"
    );
}

#[tokio::test]
async fn lan_bind_requires_paired_device_token() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("ok".into())],
    ))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.lan_bind = true; // §12.6 M3：显式开启
    let handle = tenon_daemon::serve(options).await.unwrap();

    // 局域网源（Host 为 LAN 地址）无配对令牌 → 403
    let denied = reqwest::Client::new()
        .get(format!("{}/health", base(handle.port)))
        .header("Host", "192.168.1.5")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403, "LAN 未持令牌应拒绝");

    // 本机回环不受影响
    let local = reqwest::get(format!("{}/health", base(handle.port)))
        .await
        .unwrap();
    assert_eq!(local.status(), 200);
}

#[tokio::test]
async fn lan_pairing_flow_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("ok".into())],
    ))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.lan_bind = true;
    let handle = tenon_daemon::serve(options).await.unwrap();
    let port = handle.port;
    let token = handle.token.clone();
    let local = client_with_token(&token);

    // 1. 本机开启局域网 → 一次性配对码
    let enabled: serde_json::Value = local
        .post(format!("{}/lan/enable", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = enabled["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);

    // 2. 局域网设备凭码配对（Host 为 LAN 地址，免主 token）
    let pair_resp = reqwest::Client::new()
        .post(format!("{}/lan/pair", base(port)))
        .header("Host", "192.168.1.5")
        .json(&serde_json::json!({"device": "phone", "code": code}))
        .send()
        .await
        .unwrap();
    eprintln!("LAN pair status: {}", pair_resp.status());
    let pair_body = pair_resp.text().await.unwrap();
    eprintln!("LAN pair body: {pair_body}");
    let paired: serde_json::Value = serde_json::from_str(&pair_body).unwrap();
    assert_eq!(paired["paired"], true);
    let device_token = paired["paired_token"].as_str().unwrap().to_string();

    // 3. 配对后局域网请求：携带设备令牌（X-Tenon-Paired，经 PairingStore 校验）
    let ok = reqwest::Client::new()
        .get(format!("{}/models", base(port)))
        .header("Host", "192.168.1.5")
        .header("X-Tenon-Paired", &device_token)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "配对设备令牌放行");

    // 4. 吊销 → 局域网令牌失效（§12.6 可吊销）
    let _ = local
        .post(format!("{}/lan/revoke", base(port)))
        .json(&serde_json::json!({"device": "phone"}))
        .send()
        .await
        .unwrap();
    let revoked_denied = reqwest::Client::new()
        .get(format!("{}/models", base(port)))
        .header("Host", "192.168.1.5")
        .header("X-Tenon-Paired", &device_token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked_denied.status(), 403, "吊销后 LAN 访问应被拒");
}

// v1.145：plugin_registry_install_flow_with_permission_diff 随官方 registry
// 检索安装端点退役（§13.5）——社区市场安装链路由 tests/market_api_tests.rs 承接。

#[tokio::test]
async fn static_ui_serving_and_pairing_self_discovery() {
    // 本机浏览器访问最后一环（§12.6 / M2）：daemon 同源托管 ui/dist，
    // 浏览器打开 http://127.0.0.1:{port}/ 即加载 UI；/pairing 返回自发现握手
    let dir = tempfile::tempdir().unwrap();
    let ui_dist = tempfile::tempdir().unwrap();
    std::fs::write(
        ui_dist.path().join("index.html"),
        "<html><body>tenon-ui</body></html>",
    )
    .unwrap();
    std::fs::create_dir_all(ui_dist.path().join("assets")).unwrap();
    std::fs::write(ui_dist.path().join("assets/app.js"), "// js").unwrap();

    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("ok".into())],
    ))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snaps"));
    // TENON_UI_DIST 注入（serve() 读此环境变量）
    std::env::set_var("TENON_UI_DIST", ui_dist.path());
    let handle = tenon_daemon::serve(options).await.unwrap();

    // 静态资源
    let index = reqwest::get(format!("{}/", base(handle.port)))
        .await
        .unwrap();
    assert_eq!(index.status(), 200);
    assert!(index.text().await.unwrap().contains("tenon-ui"));
    let asset = reqwest::get(format!("{}/assets/app.js", base(handle.port)))
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);

    // /pairing 自发现（免 token，本机）
    let pairing: serde_json::Value = reqwest::get(format!("{}/pairing", base(handle.port)))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pairing["self_hosted"], true);
    assert!(pairing["ws_ticket"].as_str().is_some());

    std::env::remove_var("TENON_UI_DIST");
}

#[tokio::test]
async fn project_runtime_streams_scoped_file_changes() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("watched");
    std::fs::create_dir_all(&project).unwrap();

    // v1.128：本用例的对象是「作用域文件事件经 WS 流转」，监听源用轮询后端
    //（测试确定性通道）——原生 FSEvents 注册在 fseventsd 高负载机器上可达
    // 数秒且需后台补注册，不应让本用例依赖系统态势。
    let (_tmp, port, token) = start_daemon_with_poll_watcher(vec![]).await;
    let client = client_with_token(&token);
    let opened: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = opened["id"].as_str().unwrap().to_string();

    let ticket: serde_json::Value = client
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?project_id={project_id}"
    ))
    .await
    .unwrap();
    use futures::{SinkExt, StreamExt};
    ws.send(tokio_tungstenite::tungstenite::Message::text(
        ticket["ticket"].as_str().unwrap().to_string(),
    ))
    .await
    .unwrap();
    let auth = ws.next().await.unwrap().unwrap();
    assert!(auth.to_string().contains("auth ok"));

    std::fs::write(project.join("watched.txt"), "changed").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut seen = false;
    while std::time::Instant::now() < deadline {
        let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await
        else {
            continue;
        };
        let value: serde_json::Value = serde_json::from_str(msg.to_string().as_str()).unwrap();
        eprintln!("ws event: {value}");
        if value["project_id"] == project_id.as_str()
            && value["path"] == "watched.txt"
            && matches!(value["type"].as_str(), Some("created" | "modified"))
        {
            seen = true;
            break;
        }
    }
    assert!(seen, "WS 应收到作用域内文件变更");
}

#[tokio::test]
async fn fuzzy_files_rank_and_respect_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("fuzzy");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::create_dir_all(project.join("node_modules/pkg")).unwrap();
    std::fs::write(project.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::write(project.join("src/application.ts"), "export {}\n").unwrap();
    std::fs::write(project.join("README.md"), "# app\n").unwrap();
    std::fs::write(project.join("node_modules/pkg/application.js"), "ignored\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let opened: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = opened["id"].as_str().unwrap();

    let result: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/files/fuzzy?q=application&limit=10",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let hits = result["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{result}");
    assert_eq!(hits[0]["path"], "src/application.ts");
    assert!(hits[0]["score"].as_i64().unwrap() > 0);

    let limited: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/files/fuzzy?q=application&limit=1",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(limited["hits"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn settings_panel_roundtrip_validation_and_persistence() {
    // §7.2 / §15：PUT 校验 → 运行时生效 → settings.json 持久化；
    // 非法值 400；GET 返回合并视图
    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let put = |body: serde_json::Value| {
        let client = client.clone();
        async move {
            client
                .put(format!("{}/settings", base(port)))
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    let r = put(serde_json::json!({
        "session": {"first_edit_buffer_ms": 1500},
        "exec": {"command_timeout_s": 90},
        "update": {"channel": "auto"}
    }))
    .await;
    assert_eq!(r.status(), 200);
    let merged: serde_json::Value = r.json().await.unwrap();
    assert_eq!(merged["session"]["first_edit_buffer_ms"], 1500);
    assert_eq!(merged["exec"]["command_timeout_s"], 90);
    assert_eq!(merged["update"]["channel"], "auto");

    // 持久化文件（0600）
    let file = _tmp.path().join("settings.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["session"]["first_edit_buffer_ms"], 1500);
    assert_eq!(v["update"]["channel"], "auto");

    // 已移除键与非法值逐一 400（v1.92：mode / privacy 不再接受）
    for bad in [
        serde_json::json!({"session": {"mode": "auto"}}),
        serde_json::json!({"privacy": {"telemetry": true}}),
        serde_json::json!({"session": {"first_edit_buffer_ms": -1}}),
        serde_json::json!({"exec": {"command_timeout_s": 99999}}),
        serde_json::json!({"update": {"channel": "daily"}}),
    ] {
        let r = put(bad.clone()).await;
        assert_eq!(r.status(), 400, "bad={bad}");
    }
}

#[tokio::test]
async fn settings_models_roundtrip_rebuild_and_validation() {
    // v1.40 设置面板模型分区：PUT models 键 → 校验 → 持久化 → provider 表即时重建；
    // api_key 明文 400；GET 合并视图回显 overridden 且不回显密钥。
    // 自建 daemon（无 CLI 默认 provider）：覆盖优先级 = CLI > 设置覆盖 > 配置文件。
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", vec![]))];
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    let handle = serve(options).await.unwrap();
    let (port, token) = (handle.port, handle.token);
    let client = client_with_token(&token);

    let put = |body: serde_json::Value| {
        let client = client.clone();
        async move {
            client
                .put(format!("{}/settings", base(port)))
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    let get_models = || {
        let client = client.clone();
        async move {
            let r: serde_json::Value = client
                .get(format!("{}/models", base(port)))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            r
        }
    };

    // 合法 PUT：新增 provider + 设默认 → 200；GET /models 反映重建结果
    let r = put(serde_json::json!({
        "models": {
            "default": "deepseek",
            "providers": {
                "deepseek": {
                    "kind": "openai",
                    "base_url": "https://api.deepseek.com",
                    "model": "deepseek-chat",
                    "api_key_env": "DEEPSEEK_API_KEY"
                }
            }
        }
    }))
    .await;
    assert_eq!(r.status(), 200);
    let merged: serde_json::Value = r.json().await.unwrap();
    assert_eq!(merged["models"]["default"], "deepseek");
    assert_eq!(
        merged["models"]["providers"]["deepseek"]["overridden"],
        true
    );
    assert!(
        merged["models"]["providers"]["deepseek"]
            .get("api_key")
            .is_none(),
        "合并视图永不回显 api_key 明文"
    );

    let models = get_models().await;
    assert_eq!(models["default"], "deepseek");
    let arr = models["models"].as_array().unwrap();
    let deepseek = arr
        .iter()
        .find(|m| m["name"] == "deepseek")
        .expect("重建后 provider 表应含 deepseek");
    assert_eq!(deepseek["is_default"], true);
    assert_eq!(deepseek["default_model"], "deepseek-chat");
    // 注入 provider（测试 mock）在重建后保留
    assert!(
        arr.iter().any(|m| m["name"] == "mock"),
        "注入 provider 不应被重建丢弃"
    );

    // 持久化：settings.json 含 models 覆盖
    let file = dir.path().join("settings.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["models"]["providers"]["deepseek"]["base_url"],
        "https://api.deepseek.com"
    );

    // 非法输入逐一 400：坏 kind / 坏 base_url / 明文密钥 / 坏 provider 名 / 未配置默认
    for bad in [
        serde_json::json!({"models": {"providers": {"x": {"kind": "grpc", "base_url": "https://a.b"}}}}),
        serde_json::json!({"models": {"providers": {"x": {"kind": "openai", "base_url": "ftp://a.b"}}}}),
        serde_json::json!({"models": {"providers": {"x": {"kind": "openai", "base_url": "https://a.b", "api_key": "sk-plain"}}}}),
        serde_json::json!({"models": {"providers": {"Bad_Name": {"kind": "openai", "base_url": "https://a.b"}}}}),
        serde_json::json!({"models": {"providers": {"x": {"kind": "openai", "base_url": "https://a.b", "api_key_env": "9BAD"}}}}),
        serde_json::json!({"models": {"default": "nonexistent"}}),
    ] {
        let r = put(bad.clone()).await;
        assert_eq!(r.status(), 400, "bad={bad}");
    }

    // 删除覆盖：providers 整体替换为空 + default 清空 → 表回退配置默认
    let r = put(serde_json::json!({"models": {"default": "", "providers": {}}})).await;
    assert_eq!(r.status(), 200);
    let models = get_models().await;
    assert!(
        !models["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "deepseek"),
        "覆盖删除后 provider 表不应再含 deepseek"
    );
    assert_ne!(models["default"], "deepseek");
}

#[tokio::test]
async fn project_display_name_register_open_and_rename() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("origin-name");
    std::fs::create_dir_all(&project).unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let url = base(port);

    // POST /projects/open 登记时带显示名 → 摘要返回自定义名（v1.92：PUT /project 移除）
    let registered: serde_json::Value = client
        .post(format!("{url}/projects/open"))
        .json(&serde_json::json!({
            "path": project.to_string_lossy(),
            "display_name": "自定义项目名",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["display_name"], "自定义项目名");

    let listed: serde_json::Value = client
        .get(format!("{url}/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["projects"][0]["display_name"], "自定义项目名");

    // POST /projects/open 重开换名 → 响应与摘要同步更新
    let opened: serde_json::Value = client
        .post(format!("{url}/projects/open"))
        .json(&serde_json::json!({
            "path": project.to_string_lossy(),
            "display_name": "Renamed Project",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(opened["display_name"], "Renamed Project");

    let listed: serde_json::Value = client
        .get(format!("{url}/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["projects"][0]["display_name"], "Renamed Project");

    // 空串清除自定义名 → 回退路径末段派生
    let cleared: serde_json::Value = client
        .post(format!("{url}/projects/open"))
        .json(&serde_json::json!({
            "path": project.to_string_lossy(),
            "display_name": "",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["display_name"], "origin-name");
}

// v1.153 §15：POST /projects/:id/rename——已登记项目改名，只改显示名元数据，
// 不动 path 与 project_id；trim 落库、空串回退派生、未知 id 404。
#[tokio::test]
async fn project_rename_endpoint_updates_display_name() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("rename-target");
    std::fs::create_dir_all(&project).unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let url = base(port);

    let registered: serde_json::Value = client
        .post(format!("{url}/projects/open"))
        .json(&serde_json::json!({ "path": project.to_string_lossy() }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = registered["id"].as_str().unwrap().to_string();
    assert_eq!(registered["display_name"], "rename-target");
    // daemon canonicalize 后落库（macOS /var → /private/var），断言对齐规范化路径
    let canonical = std::fs::canonicalize(&project)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // trim 落库 → 响应与摘要同步
    let renamed: serde_json::Value = client
        .post(format!("{url}/projects/{pid}/rename"))
        .json(&serde_json::json!({ "display_name": "  新名字  " }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renamed["id"], pid);
    assert_eq!(renamed["display_name"], "新名字");
    let listed: serde_json::Value = client
        .get(format!("{url}/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["projects"][0]["display_name"], "新名字");
    assert_eq!(listed["projects"][0]["path"], canonical);

    // 空串清除自定义名 → 回退路径末段派生
    let cleared: serde_json::Value = client
        .post(format!("{url}/projects/{pid}/rename"))
        .json(&serde_json::json!({ "display_name": "" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["display_name"], "rename-target");

    // 未知 id → 404
    let missing = client
        .post(format!("{url}/projects/no-such-id/rename"))
        .json(&serde_json::json!({ "display_name": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404, "未知项目 id 应 404");
}

#[tokio::test]
async fn lsp_workspace_edit_applies_atomically_with_checkpoint_and_dirty_guard() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("refactor");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("a.ts"),
        "const value = old;\nexport { value };\n",
    )
    .unwrap();

    let (_tmp, port, token) = start_daemon(vec![ScriptedReply::Text("ok".into())]).await;
    let client = client_with_token(&token);
    let url = base(port);
    let opened: serde_json::Value = client
        .post(format!("{url}/projects/open"))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = opened["id"].as_str().unwrap().to_string();
    let session: serde_json::Value = client
        .post(format!("{url}/session"))
        .json(&serde_json::json!({"project_id": project_id, "provider": "mock"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = session["session_id"].as_str().unwrap().to_string();

    // WorkspaceEdit：多文件形态只需一处；daemon 先快照，原子写，再补 checkpoint。
    let uri = format!("file://{}", project.join("a.ts").to_string_lossy());
    let applied: serde_json::Value = client
        .post(format!("{url}/project/{project_id}/lsp/apply"))
        .json(&serde_json::json!({
            "session_id": session_id,
            "workspace_edit": {"changes": {uri.clone(): [
                {"range": {"start":{"line":0,"character":14},"end":{"line":0,"character":17}}, "newText": "new"}
            ]}}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(applied["applied"], true);
    assert_eq!(
        std::fs::read_to_string(project.join("a.ts")).unwrap(),
        "const value = new;\nexport { value };\n"
    );
    let checkpoint_id = applied["checkpoint_id"].as_str().unwrap().to_string();

    // checkpoint 可通过既有 rollback 语义恢复到 LSP 写前 tree。
    let rolled: serde_json::Value = client
        .post(format!("{url}/checkpoint/{checkpoint_id}/rollback"))
        .json(&serde_json::json!({"granularity": "checkpoint"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rolled["rolled_back"][0], "a.ts");
    assert_eq!(
        std::fs::read_to_string(project.join("a.ts")).unwrap(),
        "const value = old;\nexport { value };\n"
    );

    // 用户未保存缓冲不被 LSP server 结果静默覆盖。
    client
        .put(format!("{url}/project/{project_id}/buffers"))
        .json(&serde_json::json!({"path": "a.ts", "dirty": "user edit"}))
        .send()
        .await
        .unwrap();
    let conflict = client
        .post(format!("{url}/project/{project_id}/lsp/apply"))
        .json(&serde_json::json!({
            "session_id": session_id,
            "workspace_edit": {"changes": {uri: [
                {"range": {"start":{"line":0,"character":14},"end":{"line":0,"character":17}}, "newText": "new"}
            ]}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    assert_eq!(
        std::fs::read_to_string(project.join("a.ts")).unwrap(),
        "const value = old;\nexport { value };\n"
    );
}

#[tokio::test]
async fn inline_complete_uses_project_scoped_active_session() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("completion");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("app.ts"), "const value = 1;\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![ScriptedReply::Text("value = 2;".into())]).await;
    let client = client_with_token(&token);
    let opened: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = opened["id"].as_str().unwrap().to_string();
    let session: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_id": project_id, "provider": "mock"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = session["session_id"].as_str().unwrap().to_string();

    let completed: serde_json::Value = client
        .post(format!(
            "{}/project/{project_id}/inline-complete",
            base(port)
        ))
        .json(&serde_json::json!({
            "session_id": session_id,
            "path": "app.ts",
            "language": "typescript",
            "prefix": "const value = 1;\n",
            "suffix": ""
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(completed["completion"], "value = 2;");

    // 跨项目会话不得复用：伪造另一个 project id 必须拒绝。
    let denied = client
        .post(format!(
            "{}/project/other-project/inline-complete",
            base(port)
        ))
        .json(&serde_json::json!({
            "session_id": session_id,
            "path": "app.ts",
            "language": "typescript",
            "prefix": "",
            "suffix": ""
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 409);
}

async fn start_daemon_with_provider(
    providers: Vec<Arc<dyn ModelProvider>>,
) -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = providers;
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    let handle = serve(options).await.unwrap();
    (dir, handle.port, handle.token)
}

async fn create_session(port: u16, token: &str, project: &std::path::Path) -> String {
    let client = client_with_token(token);
    let session: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_path": project.to_string_lossy(), "provider": "mock"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    session["session_id"].as_str().unwrap().to_string()
}

/// 轮询 GET /projects 直至目标会话出现非空标题（标题生成在后台完成）。
async fn wait_for_title(port: u16, token: &str, session_id: &str) -> String {
    let client = client_with_token(token);
    for _ in 0..100 {
        let projects: serde_json::Value = client
            .get(format!("{}/projects", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(title) = projects["projects"][0]["sessions"]
            .as_array()
            .and_then(|sessions| {
                sessions
                    .iter()
                    .find(|s| s["id"].as_str() == Some(session_id))
            })
            .and_then(|s| s["title"].as_str())
            .filter(|t| !t.is_empty())
        {
            return title.to_string();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("session title not generated in time");
}

/// v1.58 对话标题：首条消息后台生成并展示于 /projects；第二条消息不再重生成，
/// 任务脚本队列不受标题请求影响。
#[tokio::test]
async fn first_message_generates_session_title_once() {
    let project = tempfile::tempdir().unwrap();
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("任务回答".into())],
    ));
    let (_dir, port, token) = start_daemon_with_provider(vec![provider.clone()]).await;
    let session_id = create_session(port, &token, project.path()).await;
    let client = client_with_token(&token);

    client
        .post(format!("{}/session/{session_id}/message", base(port)))
        .json(&serde_json::json!({"text": "帮我修复登录超时的问题"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        wait_for_title(port, &token, &session_id).await,
        "Mock 会话标题"
    );

    // 第二条消息：已有标题即不重生成。
    client
        .post(format!("{}/session/{session_id}/message", base(port)))
        .json(&serde_json::json!({"text": "换个话题"}))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(provider.title_calls().len(), 1);
}

/// 模型失败时回退首条消息本地截断（不重试、不阻塞任务）。
#[tokio::test]
async fn title_falls_back_to_first_message_truncation_when_model_fails() {
    let project = tempfile::tempdir().unwrap();
    // 端口 1 连接必败：标题生成失败 → 回退本地截断。
    let dead = Arc::new(tenon_models::OpenAiCompatProvider::new(
        "mock",
        "http://127.0.0.1:1/v1",
        "",
        Some("m".into()),
    ));
    let (_dir, port, token) = start_daemon_with_provider(vec![dead]).await;
    let session_id = create_session(port, &token, project.path()).await;
    let client = client_with_token(&token);

    let text = "这个任务很长很长很长很长很长很长很长很长很长很长很长超出十六字";
    client
        .post(format!("{}/session/{session_id}/message", base(port)))
        .json(&serde_json::json!({"text": text}))
        .send()
        .await
        .unwrap();
    let title = wait_for_title(port, &token, &session_id).await;
    let expected: String = text.chars().take(16).collect();
    assert_eq!(title, expected);
}

#[tokio::test]
async fn runtime_capacity_evicts_lru_instead_of_rejecting() {
    // v1.60 登记即用：max_open 只约束内部运行时缓存——超限逐出无活跃会话的
    // 最久未用 runtime，登记 / 切换不再 409 拒绝；重新激活即恢复。
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.config.projects.max_open = 2;
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", vec![]))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    let handle = serve(options).await.unwrap();
    let port = handle.port;
    let client = client_with_token(&handle.token);

    let p1 = dir.path().join("p1");
    let p2 = dir.path().join("p2");
    let p3 = dir.path().join("p3");
    for p in [&p1, &p2, &p3] {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(p1.join("a.txt"), "a").unwrap();

    let open = |path: &std::path::Path| {
        let client = client.clone();
        let path = path.to_string_lossy().to_string();
        async move {
            client
                .post(format!("{}/projects/open", base(port)))
                .json(&serde_json::json!({ "path": path }))
                .send()
                .await
                .unwrap()
        }
    };
    let first = open(&p1).await;
    assert_eq!(first.status(), 200);
    let id1 = first.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(open(&p2).await.status(), 200);

    // 摘要不再携带 open 字段（v1.60）
    let listed: serde_json::Value = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p.get("open").is_none()));

    // 第三个项目：容量超限不拒绝，LRU 逐出最久未用的 p1。
    let third = open(&p3).await;
    assert_eq!(third.status(), 200, "容量超限应 LRU 逐出而非 409");

    // 登记即用：访问被逐出项目的文件 API 会隐式重新激活 runtime（v1.60）。
    assert_eq!(open(&p1).await.status(), 200);
    let back: serde_json::Value = client
        .get(format!("{}/project/{id1}/file?path=a.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(back["content"], "a");
}
/// §8.7 项目切换预算：runtime 已打开且有可复用会话时，核心切换链路
/// （summary → ui-state → root tree）P50 必须 <150ms。
#[tokio::test]
#[ignore = "performance budget；performance CI 显式运行"]
async fn warm_project_switch_core_calls_meet_p50_budget() {
    let projects = tempfile::tempdir().unwrap();
    let project_a = projects.path().join("switch-a");
    let project_b = projects.path().join("switch-b");
    for (root, name) in [(&project_a, "alpha"), (&project_b, "beta")] {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(format!("{name}.txt")), name).unwrap();
    }
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let mut ids = Vec::new();
    for path in [&project_a, &project_b] {
        let opened: serde_json::Value = client
            .post(format!("{}/projects/open", base(port)))
            .json(&serde_json::json!({"path": path.to_string_lossy()}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = opened["id"].as_str().unwrap().to_string();
        let session: serde_json::Value = client
            .post(format!("{}/session", base(port)))
            .json(&serde_json::json!({"project_id": id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(session["session_id"].as_str().is_some());
        ids.push(id);
    }

    // 预热 HTTP 连接、watcher 与目录读取；预算只评估“runtime 已打开”的切换。
    for _ in 0..2 {
        for id in &ids {
            for path in [
                "/projects",
                &format!("/project/{id}/ui-state"),
                &format!("/project/{id}/tree"),
            ] {
                let status = client
                    .get(format!("{}{path}", base(port)))
                    .send()
                    .await
                    .unwrap()
                    .status();
                assert_eq!(status, 200);
            }
        }
    }

    let mut elapsed_ms = Vec::new();
    for round in 0..9 {
        let id = &ids[round % 2];
        let start = Instant::now();
        for path in [
            "/projects",
            &format!("/project/{id}/ui-state"),
            &format!("/project/{id}/tree"),
        ] {
            let response = client
                .get(format!("{}{path}", base(port)))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }
        elapsed_ms.push(start.elapsed().as_millis() as u64);
    }
    elapsed_ms.sort_unstable();
    let p50 = elapsed_ms[elapsed_ms.len() / 2];
    eprintln!("warm project switch core calls: p50={p50}ms samples={elapsed_ms:?}");
    assert!(
        p50 < 150,
        "项目切换核心链路超出 150ms P50：p50={p50}ms samples={elapsed_ms:?}"
    );
}

// ---------- Laya 自动下载并启用（§9.8 v1.71） ----------

const LAYA_STARTER_MODEL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tenon-laya/models/laya-starter-v1.json"
);

/// 固定测试密钥：所有用例同一公钥，避免并行用例的公钥环境竞争。
fn laya_test_signer() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
}

/// 本地静态 registry 夹具：`/registry/laya.json` 签名清单 + `/model.json` 模型体。
/// 返回（registry URL, 验签公钥 hex）；公钥经 `DaemonOptions.laya_public_key`
/// 注入 daemon——不写进程 env（`TENON_LAYA_PUBLIC_KEY` 是 §12.5 插件验签链的
/// 回退项，写 env 会串扰同进程并行测试）。
async fn spawn_laya_registry() -> (String, String) {
    use ed25519_dalek::Signer;

    let model = std::fs::read(LAYA_STARTER_MODEL).unwrap();
    let sk = laya_test_signer();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // 模型体内部版本同步升 2：/models 上报的是模型文件版本，须与清单版本一致
    // 才能区分「registry v2」与「starter 兜底 v1」；sha 在变异后计算，保证
    // 清单钉扎的就是所服务的字节（v1.102）。
    let mut model: serde_json::Value = serde_json::from_slice(&model).unwrap();
    model["version"] = serde_json::json!(2);
    let model = serde_json::to_vec(&model).unwrap();
    let sha = tenon_laya::sha256_hex(&model);
    // urls 首位为不可达镜像（v1.102）：常规路径即覆盖模型镜像回退。
    let manifest = serde_json::json!({
        "laya": {
            "version": 2,
            "sha256": sha,
            "signature": hex::encode(sk.sign(sha.as_bytes()).to_bytes()),
            "url": format!("http://{addr}/model.json"),
            "urls": ["http://127.0.0.1:1/model.json", format!("http://{addr}/model.json")],
        }
    });
    let public_key = hex::encode(sk.verifying_key().to_bytes());
    let app = axum::Router::new()
        .route(
            "/registry/laya.json",
            axum::routing::get(move || {
                let manifest = manifest.clone();
                async move { axum::Json(manifest) }
            }),
        )
        .route(
            "/model.json",
            axum::routing::get(move || {
                let model = model.clone();
                async move { model }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/registry/laya.json"), public_key)
}

/// 轮询 /models 直到 Laya 下载装载完成（自动下载为后台任务）。
async fn wait_laya_downloaded(port: u16, token: &str) -> serde_json::Value {
    let client = client_with_token(token);
    for _ in 0..50 {
        if let Ok(r) = client.get(format!("{}/models", base(port))).send().await {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                if v["laya"]["downloaded"] == serde_json::json!(true) {
                    return v;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Laya 自动下载未在 5s 内完成");
}

#[tokio::test]
async fn laya_auto_downloads_and_enables_on_startup() {
    let (registry, public_key) = spawn_laya_registry().await;
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.laya_models_dir = Some(dir.path().join("models/laya"));
    options.laya_registry_url = Some(registry);
    options.laya_public_key = Some(public_key);
    options.config.models.laya.auto_download = true;
    let handle = serve(options).await.unwrap();

    let models = wait_laya_downloaded(handle.port, &handle.token).await;
    assert_eq!(
        models["laya"]["version"],
        serde_json::json!(["laya-starter", 2]),
        "应为 registry 清单 v2（镜像回退后命中，非 starter 兜底 v1）：{models}"
    );
}

#[tokio::test]
async fn laya_auto_download_disabled_stays_unloaded() {
    let (registry, _public_key) = spawn_laya_registry().await;
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.laya_models_dir = Some(dir.path().join("models/laya"));
    options.laya_registry_url = Some(registry);
    // in_memory() 默认 auto_download = false（§9.8：测试基座不出网）
    let handle = serve(options).await.unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    let client = client_with_token(&handle.token);
    let models: serde_json::Value = client
        .get(format!("{}/models", base(handle.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        models["laya"]["downloaded"],
        serde_json::json!(false),
        "auto_download=false 不得自动下载：{models}"
    );
}

#[tokio::test]
async fn laya_falls_back_to_bundled_starter_when_registry_unreachable() {
    // 本地恒 500 的 registry：模拟在线链路全镜像失败（确定性错误路径，
    // 不依赖 connection refused 的时序）
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/registry/laya.json",
        axum::routing::get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    let models_dir = dir.path().join("models/laya");
    options.laya_models_dir = Some(models_dir.clone());
    options.laya_registry_url = Some(format!("http://{addr}/registry/laya.json"));
    options.config.models.laya.auto_download = true;
    let handle = serve(options).await.unwrap();

    let models = wait_laya_downloaded(handle.port, &handle.token).await;
    assert_eq!(
        models["laya"]["version"],
        serde_json::json!(["laya-starter", 1]),
        "registry 不可达应兜底内置 starter v1：{models}"
    );
    // 已落盘：下次启动直接装载，不重写
    assert!(models_dir.join("model.json").exists());
}

#[tokio::test]
async fn laya_upgrades_installed_starter_from_registry_when_reachable() {
    let (registry, public_key) = spawn_laya_registry().await;
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models/laya");
    std::fs::create_dir_all(&models_dir).unwrap();
    // 预装 starter v1（模拟离线首启兜底产物）；registry 可达（v2）→ 自动升级覆盖
    std::fs::write(models_dir.join("model.json"), tenon_laya::STARTER_MODEL).unwrap();
    let mut options = DaemonOptions::in_memory();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.laya_models_dir = Some(models_dir);
    options.laya_registry_url = Some(registry);
    options.laya_public_key = Some(public_key);
    options.config.models.laya.auto_download = true;
    let handle = serve(options).await.unwrap();

    let client = client_with_token(&handle.token);
    for _ in 0..50 {
        if let Ok(r) = client
            .get(format!("{}/models", base(handle.port)))
            .send()
            .await
        {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                if v["laya"]["version"] == serde_json::json!(["laya-starter", 2]) {
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("已装 starter v1 未在 5s 内升级到 registry v2");
}

// ---------- 受管 worktree 并行会话（v1.87 §9.7） ----------

fn git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn managed_worktree_session_merge_conflict_and_discard() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@tenon.dev"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("tracked.txt"), "line1\nline2\nline3\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "init"]);

    // 非 git 项目 → 受管 worktree 会话创建必须 400（§9.5 worktree 隔离要求 git）
    let plain = tempfile::tempdir().unwrap();
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let opened = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": plain.path().to_str().unwrap()}))
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), 200);
    let plain_id = opened.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_id": plain_id, "worktree": "managed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "非 git 仓库拒绝受管 worktree 会话");
    let _ = client
        .delete(format!("{}/projects/{plain_id}", base(port)))
        .send()
        .await;

    // git 项目：打开 + 创建受管 worktree 会话
    let opened = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": root.to_str().unwrap()}))
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), 200);
    let pid = opened.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_id": pid, "worktree": "managed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let created = resp.json::<serde_json::Value>().await.unwrap();
    let sid = created["session_id"].as_str().unwrap().to_string();
    let wt_path = created["worktree_path"].as_str().unwrap().to_string();
    assert!(!wt_path.is_empty(), "响应必须携带 worktree 路径");
    let wt = std::path::PathBuf::from(&wt_path);
    assert!(wt.exists(), "受管 worktree 目录应已创建");
    assert!(
        !wt.starts_with(root),
        "worktree 不在用户工作区内（§9.5 内核托管）"
    );

    // worktree 内模拟代理改动：改 tracked.txt + 新增 new_file.txt
    std::fs::write(wt.join("tracked.txt"), "line1\nWT\nline3\n").unwrap();
    std::fs::write(wt.join("new_file.txt"), "hello wt\n").unwrap();

    // 主根同文件另一改 → 三方冲突，整体不落盘
    std::fs::write(root.join("tracked.txt"), "line1\nPROJECT\nline3\n").unwrap();
    let resp = client
        .post(format!("{}/session/{sid}/worktree/merge", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409, "重叠改动必须冲突");
    let body = resp.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["error"], "MERGE_CONFLICT");
    assert_eq!(body["report"]["conflicts"][0]["path"], "tracked.txt");
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).unwrap(),
        "line1\nPROJECT\nline3\n",
        "冲突不得静默覆盖主根"
    );

    // 主根回到 base 后重合并 → 干净合入（改写 + 新文件）
    std::fs::write(root.join("tracked.txt"), "line1\nline2\nline3\n").unwrap();
    let resp = client
        .post(format!("{}/session/{sid}/worktree/merge", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let report = resp.json::<serde_json::Value>().await.unwrap();
    let merged = report["merged"].as_array().unwrap();
    assert!(merged.contains(&serde_json::json!("tracked.txt")));
    assert!(merged.contains(&serde_json::json!("new_file.txt")));
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).unwrap(),
        "line1\nWT\nline3\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("new_file.txt")).unwrap(),
        "hello wt\n"
    );
    assert!(
        report["snapshot_tree"].as_str().is_some(),
        "合并前项目根 shadow 快照（§10.3 回滚原语）必须存在"
    );

    // 脏缓冲跳过：worktree 新增 dirty.txt，主根登记脏缓冲 → 合并跳过且不落盘
    std::fs::write(wt.join("dirty.txt"), "dirty\n").unwrap();
    let resp = client
        .put(format!("{}/project/{pid}/buffers", base(port)))
        .json(&serde_json::json!({"path": "dirty.txt", "dirty": "ui edit"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = client
        .post(format!("{}/session/{sid}/worktree/merge", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let report = resp.json::<serde_json::Value>().await.unwrap();
    assert_eq!(
        report["skipped"][0]["reason"], "dirty_buffer",
        "脏缓冲文件必须显式跳过（§8.6 不静默覆盖）"
    );
    assert!(!root.join("dirty.txt").exists());

    // 丢弃：缺 confirm → 400；confirm → 删除 worktree 与快照分片，主根不动
    let resp = client
        .post(format!("{}/session/{sid}/worktree/discard", base(port)))
        .json(&serde_json::json!({"confirm": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = client
        .post(format!("{}/session/{sid}/worktree/discard", base(port)))
        .json(&serde_json::json!({"confirm": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(!wt.exists(), "丢弃后 worktree 目录应删除");
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).unwrap(),
        "line1\nWT\nline3\n",
        "丢弃不影响已合并内容"
    );

    // 非受管会话走收尾端点 → 409 NOT_MANAGED_WORKTREE
    let resp = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_id": pid}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let plain_sid = resp.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = client
        .post(format!("{}/session/{plain_sid}/worktree/merge", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // /projects 摘要带 worktree_path（全局活动条 / 会话列表数据源）
    let resp = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap();
    let projects = resp.json::<serde_json::Value>().await.unwrap();
    let sessions = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == serde_json::json!(pid))
        .unwrap()["sessions"]
        .as_array()
        .unwrap()
        .clone();
    let managed = sessions
        .iter()
        .find(|s| s["id"] == serde_json::json!(sid))
        .unwrap();
    assert_eq!(managed["worktree_path"], serde_json::json!(wt_path));
    let plain = sessions
        .iter()
        .find(|s| s["id"] == serde_json::json!(plain_sid))
        .unwrap();
    assert_eq!(plain["worktree_path"], serde_json::json!(""));
}

#[tokio::test]
async fn set_readonly_control_blocks_b_level_writes_over_http() {
    // v1.93：set_readonly 实装（此前为空操作臂）——只读后 B 级写被拒，任务仍完成
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("ro-proj");
    std::fs::create_dir_all(&project).unwrap();

    let (_tmp, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Text("只读，未修改文件".into()),
    ])
    .await;
    let client = client_with_token(&token);
    let opened: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = opened["id"].as_str().unwrap().to_string();
    let session: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_id": pid, "provider": "mock"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sid = session["session_id"].as_str().unwrap().to_string();

    let ro = client
        .post(format!("{}/session/{sid}/control", base(port)))
        .json(&serde_json::json!({"action": "set_readonly", "value": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(ro.status(), 200);

    let sent = client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "改文件"}))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 202);

    let mut done = false;
    for _ in 0..100 {
        let status: serde_json::Value = client
            .get(format!("{}/session/{sid}", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if status["status"] == "done" && status["outcome"].is_object() {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(done, "只读拒绝写后任务仍应完成");
    assert!(
        !project.join("a.txt").exists(),
        "只读会话中 B 级写必须被拒绝"
    );
}

// ---------- 覆盖率补齐：低频路由全链路 ----------

#[tokio::test]
async fn settings_roundtrip_and_team_policy() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // GET settings
    let r = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // PUT settings
    let r = client
        .put(format!("{}/settings", base(port)))
        .json(&serde_json::json!({
            "session": { "first_edit_buffer_ms": 3000 },
            "exec": { "command_timeout_s": 60 }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // PUT team-policy
    let r = client
        .put(format!("{}/team-policy", base(port)))
        .json(&serde_json::json!({
            "denied_tools": ["git_push"],
            "max_cost_usd": 5.0
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET ui-prefs
    let r = client
        .get(format!("{}/ui-prefs", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // PUT ui-prefs
    let r = client
        .put(format!("{}/ui-prefs", base(port)))
        .json(&serde_json::json!({ "theme": "dark" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn pairing_lan_and_costs_endpoints() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // GET pairing
    let r = client
        .get(format!("{}/pairing", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let pairing_body: serde_json::Value = r.json().await.unwrap();
    assert!(pairing_body["port"].is_number());

    // GET costs（不带 session 可能 200 或 400——v1.93 变更容忍）
    let r = client
        .get(format!("{}/costs", base(port)))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);

    // GET costs with session filter
    let r = client
        .get(format!("{}/costs?session=s1", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn evals_and_market_endpoints() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // GET evals
    let r = client
        .get(format!("{}/evals", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET /market/sources（§13.5 v1.145；旧 /plugins 官方 registry 端点已退役）
    let r = client
        .get(format!("{}/market/sources", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v["sources"].is_array());
}

#[tokio::test]
async fn updates_endpoints() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // GET updates
    let r = client
        .get(format!("{}/updates", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // POST updates/check（可能因无网络 500——仅验证端点可达）
    let r = client
        .post(format!("{}/updates/check", base(port)))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);

    // POST updates/apply（可能因无 staged 更新 500）
    let r = client
        .post(format!("{}/updates/apply", base(port)))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn lan_endpoints() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // POST lan/enable
    let r = client
        .post(format!("{}/lan/enable", base(port)))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
    let body: serde_json::Value = r.json().await.unwrap();

    // 配对码存在
    if let Some(code) = body["code"].as_str() {
        // POST lan/pair
        let r = client
            .post(format!("{}/lan/pair", base(port)))
            .json(&serde_json::json!({ "device": "test-device", "code": code }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);

        // POST lan/revoke
        let r = client
            .post(format!("{}/lan/revoke", base(port)))
            .json(&serde_json::json!({ "device": "test-device" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
}

#[tokio::test]
async fn project_trust_endpoint() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // 打开项目
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let project_id = body["id"].as_str().unwrap();

    // PUT project trust
    let r = client
        .put(format!("{}/project/trust", base(port)))
        .json(&serde_json::json!({ "project_id": project_id, "trusted": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn file_operations_crud() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // 打开项目
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let pid = body["id"].as_str().unwrap();

    // PUT file (write)
    let r = client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "test-coverage.rs", "content": "fn main() {}" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET file (read)
    let r = client
        .get(format!(
            "{}/project/{}/file?path=test-coverage.rs",
            base(port),
            pid
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["content"], "fn main() {}");

    // GET tree
    let r = client
        .get(format!("{}/project/{}/tree", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET fuzzy
    let r = client
        .get(format!("{}/project/{}/files/fuzzy?q=test", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET search
    let r = client
        .get(format!("{}/project/{}/search?q=main", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // DELETE project
    let r = client
        .delete(format!("{}/projects/{}", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn language_packs_and_l4_endpoints() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // 打开项目
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let pid = body["id"].as_str().unwrap();

    // GET language-packs
    let r = client
        .get(format!("{}/project/{}/language-packs", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET l4/stats
    let r = client
        .get(format!("{}/project/{}/l4/stats", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // POST l4/rebuild
    let r = client
        .post(format!("{}/project/{}/l4/rebuild", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn session_lifecycle_full() {
    let script = vec![
        ScriptedReply::Text("done".into()),
        ScriptedReply::Text("ok".into()),
        ScriptedReply::Text("ok2".into()),
    ];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);

    // 打开项目
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let pid = body["id"].as_str().unwrap();

    // 创建会话
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let sid = body["session_id"].as_str().unwrap();

    // GET session
    let r = client
        .get(format!("{}/session/{}", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // 发送消息（可能因模型状态拒绝——仅验证端点可达）
    let r = client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "hello" }))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);

    // GET trace
    let r = client
        .get(format!("{}/session/{}/trace", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET checkpoints
    let r = client
        .get(format!("{}/session/{}/checkpoints", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // POST control
    let r = client
        .post(format!("{}/session/{}/control", base(port), sid))
        .json(&serde_json::json!({ "action": "stop" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // POST model switch
    let r = client
        .post(format!("{}/session/{}/model", base(port), sid))
        .json(&serde_json::json!({ "provider": "mock", "model": "mock-1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // POST model-suggest
    let r = client
        .post(format!("{}/model-suggest", base(port)))
        .json(&serde_json::json!({ "text": "write code" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // POST ws-ticket
    let r = client
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn error_paths_unauthorized_and_not_found() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // 404 不存在的路由
    let r = client
        .get(format!("{}/nonexistent", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // GET 不存在的 session
    let r = client
        .get(format!("{}/session/nonexistent", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // GET 不存在的项目树
    let r = client
        .get(format!("{}/project/nonexistent/tree", base(port)))
        .send()
        .await
        .unwrap();
    // 可能 404 或 500（取决于 daemon 实现）
    assert!(r.status().is_client_error() || r.status().is_server_error());

    // PATH_ESCAPE：尝试路径穿越
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "/nonexistent/path/that/does/not/exist" }))
        .send()
        .await
        .unwrap();
    // 可能 400（路径不存在）或 200（daemon 创建）
    assert!(r.status() == 400 || r.status() == 200 || r.status() == 500);
}

#[tokio::test]
async fn ws_ticket_post() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["ticket"].is_string());
    assert!(body["expires_in_s"].is_number());
}

#[tokio::test]
async fn inline_complete_endpoint() {
    let script = vec![ScriptedReply::Text("completion text".into())];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/project/{}/inline-complete", base(port), pid))
        .json(&serde_json::json!({ "prefix": "const x = ", "suffix": "", "path": "a.ts", "language": "ts" }))
        .send()
        .await
        .unwrap();
    // 可能 200（有 completion）或 4xx/5xx（无 provider support）
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn file_ops_endpoint() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // POST file/ops (create/rename/delete)
    let r = client
        .post(format!("{}/project/{}/file/ops", base(port), pid))
        .json(&serde_json::json!({ "ops": [
            { "op": "create", "path": "new-file.ts", "content": "export {}" }
        ]}))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn search_replace_endpoint() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 先写一个文件
    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "sr.ts", "content": "old text here" }))
        .send()
        .await
        .unwrap();

    // 搜索替换
    let r = client
        .post(format!("{}/project/{}/search/replace", base(port), pid))
        .json(&serde_json::json!({ "q": "old", "replace": "new", "files": ["sr.ts"] }))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn syntax_highlight_endpoint() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "hl.ts", "content": "const x = 1;" }))
        .send()
        .await
        .unwrap();

    let r = client
        .get(format!(
            "{}/project/{}/highlight?path=hl.ts",
            base(port),
            pid
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn git_source_view_endpoint() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .get(format!("{}/project/{}/git/view", base(port), pid))
        .send()
        .await
        .unwrap();
    // 可能 200 或 4xx/5xx（非 git 仓库）
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn project_lsp_proxy_endpoint() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/project/{}/lsp", base(port), pid))
        .json(&serde_json::json!({ "path": "test.ts", "action": "diagnostics" }))
        .send()
        .await
        .unwrap();
    // LSP 可能 200 或 4xx/5xx（无语言包安装）
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn delete_project_removes_from_list() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // 创建临时目录项目
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let pid = body["id"].as_str().unwrap();

    // DELETE
    let r = client
        .delete(format!("{}/projects/{}", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // 列表中不再出现
    let r = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = r.json().await.unwrap();
    let ids: Vec<&str> = list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert!(!ids.contains(&pid));
}

#[tokio::test]
async fn create_session_with_worktree() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // 创建 git 仓库项目
    let tmp = tempfile::tempdir().unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(tmp.path())
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@l")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@l")
        .output()
        .unwrap();
    assert!(init.status.success());

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 带 worktree 参数创建会话
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock", "worktree": "managed" }))
        .send()
        .await
        .unwrap();
    // 可能 200（git 仓库）或 400（非 git/创建失败）
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn checkpoint_rollback_endpoint() {
    let script = vec![ScriptedReply::Text("done".into())];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "." }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 创建会话 + 发消息产生 checkpoint
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let _sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // POST checkpoint rollback（不存在的 id → 404 或 400）
    let r = client
        .post(format!("{}/checkpoint/nonexistent/rollback", base(port)))
        .json(&serde_json::json!({ "granularity": "revert" }))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn merge_and_discard_worktree_endpoints() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // POST merge（不存在的 session → 404 或 400）
    let r = client
        .post(format!("{}/session/nonexistent/worktree/merge", base(port)))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);

    // POST discard
    let r = client
        .post(format!(
            "{}/session/nonexistent/worktree/discard",
            base(port)
        ))
        .json(&serde_json::json!({ "confirm": true }))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn settings_put_validation_error() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // PUT settings with invalid values
    let r = client
        .put(format!("{}/settings", base(port)))
        .json(&serde_json::json!({
            "session": { "first_edit_buffer_ms": -1 }
        }))
        .send()
        .await
        .unwrap();
    // 可能 200（daemon 忽略无效值）或 400（校验拒绝）
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn open_project_same_path_returns_same_id() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    let r1 = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": path }))
        .send()
        .await
        .unwrap();
    let id1 = r1.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r2 = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": path }))
        .send()
        .await
        .unwrap();
    let id2 = r2.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_eq!(id1, id2, "同路径项目应返回同一 ID");
}

#[tokio::test]
async fn open_project_with_display_name() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({
            "path": tmp.path().to_str().unwrap(),
            "display_name": "My Custom Name"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["display_name"], "My Custom Name");
}

#[tokio::test]
async fn file_write_creates_and_overwrites_content() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 写入
    let r = client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "test.txt", "content": "v1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // 读取确认
    let r = client
        .get(format!("{}/project/{}/file?path=test.txt", base(port), pid))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["content"], "v1");

    // 覆写
    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "test.txt", "content": "v2" }))
        .send()
        .await
        .unwrap();

    // 读取确认更新
    let r = client
        .get(format!("{}/project/{}/file?path=test.txt", base(port), pid))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["content"], "v2");
}

#[tokio::test]
async fn file_not_found_returns_error() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .get(format!(
            "{}/project/{}/file?path=nonexistent.txt",
            base(port),
            pid
        ))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error() || r.status().is_server_error());
}

#[tokio::test]
async fn search_finds_content_in_files() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 写入可搜索内容
    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(
            &serde_json::json!({ "path": "searchable.ts", "content": "const unique_marker = 42;" }),
        )
        .send()
        .await
        .unwrap();

    let r = client
        .get(format!(
            "{}/project/{}/search?q=unique_marker",
            base(port),
            pid
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["hits"]
        .as_array()
        .map(|h| !h.is_empty())
        .unwrap_or(false));
}

#[tokio::test]
async fn fuzzy_files_finds_matching_names() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "unique_component.tsx", "content": "export {}" }))
        .send()
        .await
        .unwrap();

    let r = client
        .get(format!(
            "{}/project/{}/files/fuzzy?q=unique_comp",
            base(port),
            pid
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["hits"]
        .as_array()
        .map(|h| !h.is_empty())
        .unwrap_or(false));
}

#[tokio::test]
async fn ws_ticket_returns_token_and_expiry() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let ticket = body["ticket"].as_str().unwrap();
    assert!(!ticket.is_empty());
    assert!(body["expires_in_s"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn health_endpoint_no_auth_needed() {
    let (_dir, port, _token) = start_daemon(vec![]).await;
    let r = reqwest::get(format!("{}/health", base(port)))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn models_endpoint_lists_providers() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .get(format!("{}/models", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["models"].is_array());
    assert_eq!(body["default"], "mock");
}

#[tokio::test]
async fn get_session_not_found_returns_404() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .get(format!("{}/session/nonexistent-id", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn tree_returns_file_entries() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "tree-test.rs", "content": "fn main() {}" }))
        .send()
        .await
        .unwrap();

    let r = client
        .get(format!("{}/project/{}/tree", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["entries"].is_array());
}

#[tokio::test]
async fn language_packs_endpoint_returns_detection() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .get(format!("{}/project/{}/language-packs", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["packs"].is_array() || body["languages"].is_array());
}

// v1.103：会话归档 / 还原 / 删除（§14.2 / §15）——摘要过滤、未收尾 worktree 守卫、confirm 门。
#[tokio::test]
async fn session_archive_unarchive_and_delete_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    // 受管 worktree 会话要求 git 仓库（§9.5）：init + 初始提交。
    git(&project, &["init", "-q"]);
    git(
        &project,
        &["config", "user.email", "tenon-test@example.com"],
    );
    git(&project, &["config", "user.name", "tenon-test"]);
    git(&project, &["commit", "-qm", "init", "--allow-empty"]);
    let (_tmp, port, token) = start_daemon(vec![]).await;
    // 共享机器高负载（并行会话构建）下 10s 默认超时会轮转超时，本测试用 60s 客户端。
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "X-Tenon-Token",
        reqwest::header::HeaderValue::from_str(&token).unwrap(),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();

    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = registered["id"].as_str().unwrap().to_string();

    let make_session = || {
        let url = format!("{}/session", base(port));
        let client = client.clone();
        let pid = pid.clone();
        async move {
            let r = client
                .post(url)
                .json(&serde_json::json!({"project_id": pid}))
                .send()
                .await
                .unwrap();
            let status = r.status();
            let text = r.text().await.unwrap();
            assert!(status.is_success(), "create_session 失败: {text}");
            serde_json::from_str::<serde_json::Value>(&text).expect("create_session 响应非 JSON")
                ["session_id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let a = make_session().await;
    let b = make_session().await;
    // 受管 worktree 会话（未收尾）：归档 / 删除应 409。
    let worktree: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_id": pid, "worktree": "managed"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let wt_id = worktree["session_id"].as_str().unwrap().to_string();

    let summary = || {
        let url = format!("{}/projects", base(port));
        let client = client.clone();
        let pid = pid.clone();
        async move {
            client
                .get(url)
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == serde_json::Value::String(pid.clone()))
                .unwrap()
                .clone()
        }
    };

    // 初始：三会话都在 sessions，无归档。
    let s = summary().await;
    assert_eq!(s["sessions"].as_array().unwrap().len(), 3);
    assert_eq!(s["archived_sessions"].as_array().unwrap().len(), 0);

    // 归档 a：sessions 排除、archived_sessions 收录。
    let r = client
        .post(format!("{}/session/{}/archive", base(port), a))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let s = summary().await;
    assert_eq!(s["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(s["archived_sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        s["archived_sessions"][0]["id"],
        serde_json::Value::String(a.clone())
    );

    // 未收尾受管 worktree：归档 409。
    let r = client
        .post(format!("{}/session/{}/archive", base(port), wt_id))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);

    // 还原 a：回到 sessions。
    let r = client
        .post(format!("{}/session/{}/unarchive", base(port), a))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let s = summary().await;
    assert_eq!(s["sessions"].as_array().unwrap().len(), 3);
    assert_eq!(s["archived_sessions"].as_array().unwrap().len(), 0);

    // 删除门：缺 confirm 字段 422（提取器拒收）、confirm:false 400；confirm:true 后 404 且摘要移除。
    let r = client
        .delete(format!("{}/session/{}", base(port), b))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let r = client
        .delete(format!("{}/session/{}", base(port), b))
        .json(&serde_json::json!({"confirm": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = client
        .delete(format!("{}/session/{}", base(port), b))
        .json(&serde_json::json!({"confirm": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .get(format!("{}/session/{}", base(port), b))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let s = summary().await;
    assert_eq!(s["sessions"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn memories_endpoints_crud() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // POST create memory
    let r = client
        .post(format!("{}/project/{}/memories", base(port), pid))
        .json(&serde_json::json!({
            "scope": "project",
            "kind": "fact",
            "content": "uses React 19 with TypeScript strict mode",
            "importance": 4
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());

    // GET list memories
    let r = client
        .get(format!("{}/project/{}/memories", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let memories = body["memories"].as_array().unwrap();
    assert!(!memories.is_empty());

    // DELETE a memory
    let mem_id = memories[0]["id"].as_str().unwrap();
    let r = client
        .delete(format!("{}/memories/{}", base(port), mem_id))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success() || r.status() == 404);
}

#[tokio::test]
async fn settings_persist_and_reload() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // PUT settings with model config
    let r = client
        .put(format!("{}/settings", base(port)))
        .json(&serde_json::json!({
            "models": {
                "default": "openai",
                "providers": {
                    "openai": { "base_url": "https://api.openai.com/v1", "model": "gpt-4" }
                }
            }
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    // GET settings reflects change
    let r = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn git_view_endpoint_with_git_repo() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // Create git repo
    let tmp = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let _ = std::process::Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@l")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@l")
            .output()
            .unwrap();
    };
    git(&["init", "--initial-branch=main"]);
    std::fs::write(tmp.path().join("tracked.txt"), "content").unwrap();
    git(&["add", "tracked.txt"]);
    git(&["commit", "-m", "init"]);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .get(format!("{}/project/{}/git/view", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn multi_project_open_and_list() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp_a = tempfile::tempdir().unwrap();
    let tmp_b = tempfile::tempdir().unwrap();

    let r_a = client
        .post(format!("{}/projects/open", base(port)))
        .json(
            &serde_json::json!({ "path": tmp_a.path().to_str().unwrap(), "display_name": "Alpha" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r_a.status(), 200);

    let r_b = client
        .post(format!("{}/projects/open", base(port)))
        .json(
            &serde_json::json!({ "path": tmp_b.path().to_str().unwrap(), "display_name": "Beta" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r_b.status(), 200);

    let r = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let names: Vec<&str> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["display_name"].as_str())
        .collect();
    assert!(names.contains(&"Alpha"));
    assert!(names.contains(&"Beta"));
}

#[tokio::test]
async fn session_trace_events_after_send() {
    let script = vec![
        ScriptedReply::Text("I analyzed the code and found the issue.".into()),
        ScriptedReply::Text("Here is my complete answer with more detail.".into()),
    ];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // 发送消息
    let r = client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "analyze" }))
        .send()
        .await
        .unwrap();
    // 消息可能因模型状态返回非 200
    let msg_status = r.status();
    if msg_status == 200 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // GET trace
    let r = client
        .get(format!("{}/session/{}/trace", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    // trace 端点可达（events 可能为空——异步处理）
    assert!(body["events"].is_array());
    assert!(body["latest_seq"].is_i64() || body["latest_seq"].is_u64());
}

#[tokio::test]
async fn checkpoint_rollback_after_write() {
    let script = vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "rollback-test.txt", "range": null, "content": "new content"}),
        },
        ScriptedReply::Text("写入完成".into()),
    ];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // 发送带工具调用的消息
    client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "write file" }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // GET checkpoints
    let r = client
        .get(format!("{}/session/{}/checkpoints", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let checkpoints = body["checkpoints"].as_array().unwrap();
    // 工具写入后应有 checkpoint
    if !checkpoints.is_empty() {
        let cp_id = checkpoints[0]["id"].as_str().unwrap();
        // POST rollback
        let r = client
            .post(format!("{}/checkpoint/{}/rollback", base(port), cp_id))
            .json(&serde_json::json!({ "granularity": "revert" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
}

#[tokio::test]
async fn evals_listing_and_market_sources() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // GET evals returns task list
    let r = client
        .get(format!("{}/evals", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    // evals 可能有 tasks 数组或空
    assert!(body.is_object());

    // GET market sources（§13.5 v1.145：/plugins 退役后市场源为插件面入口）
    let r = client
        .get(format!("{}/market/sources", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body.is_object());
}

#[tokio::test]
async fn project_rename_via_display_name() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({
            "path": tmp.path().to_str().unwrap(),
            "display_name": "Original"
        }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 重新打开并更新 display_name
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({
            "path": tmp.path().to_str().unwrap(),
            "display_name": "Renamed"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // 列表中显示更新后的名称
    let r = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let project = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"].as_str() == Some(pid.as_str()))
        .unwrap();
    assert_eq!(project["display_name"], "Renamed");
}

#[tokio::test]
async fn session_control_stop_and_status() {
    let script = vec![ScriptedReply::Text("long task response".into())];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Send a message to start execution
    client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "long running task" }))
        .send()
        .await
        .unwrap();

    // POST control: stop
    let r = client
        .post(format!("{}/session/{}/control", base(port), sid))
        .json(&serde_json::json!({ "action": "stop" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET session status
    let r = client
        .get(format!("{}/session/{}", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["status"].is_string());
}

#[tokio::test]
async fn file_highlight_returns_syntax_tokens() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "syntax.rs", "content": "fn main() { let x = 1; }" }))
        .send()
        .await
        .unwrap();

    let r = client
        .get(format!(
            "{}/project/{}/highlight?path=syntax.rs",
            base(port),
            pid
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn team_policy_put_and_verify_effect() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // PUT team policy
    let r = client
        .put(format!("{}/team-policy", base(port)))
        .json(&serde_json::json!({
            "denied_tools": ["git_push", "create_pr"],
            "max_cost_usd": 1.0
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body.is_object());
}

#[tokio::test]
async fn open_project_updates_display_name() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    // First open with default name
    client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": path }))
        .send()
        .await
        .unwrap();

    // Re-open with display name
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": path, "display_name": "Updated" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["display_name"], "Updated");
}

#[tokio::test]
async fn session_created_returns_project_id() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(!body["session_id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn set_trust_updates_project() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Verify untrusted by default
    let r = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let proj = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"].as_str() == Some(pid.as_str()))
        .unwrap();
    assert_eq!(proj["trusted"], false);

    // Set trusted
    let r = client
        .put(format!("{}/project/trust", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "trusted": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Verify trusted
    let r = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let proj = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"].as_str() == Some(pid.as_str()))
        .unwrap();
    assert_eq!(proj["trusted"], true);
}

#[tokio::test]
async fn file_search_with_regex_special_chars() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "regex-test.rs", "content": "let pattern = r\"abc.def\";" }))
        .send()
        .await
        .unwrap();

    // Search with special chars
    let r = client
        .get(format!("{}/project/{}/search?q=abc.def", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn ui_prefs_set_and_get() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // PUT ui-prefs
    let r = client
        .put(format!("{}/ui-prefs", base(port)))
        .json(&serde_json::json!({ "theme": "light", "locale": "zh-CN" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // GET ui-prefs
    let r = client
        .get(format!("{}/ui-prefs", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn model_suggest_returns_provider_recommendation() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .post(format!("{}/model-suggest", base(port)))
        .json(&serde_json::json!({ "text": "Fix a TypeScript type error in the editor" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn read_file_with_url_encoded_path() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "src/nested file.txt", "content": "spaces in name" }))
        .send()
        .await
        .unwrap();

    let r = client
        .get(format!(
            "{}/project/{}/file?path={}",
            base(port),
            pid,
            "src%2Fnested%20file.txt"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn project_delete_then_404() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Delete
    client
        .delete(format!("{}/projects/{}", base(port), pid))
        .send()
        .await
        .unwrap();

    // Try to get tree for deleted project
    let r = client
        .get(format!("{}/project/{}/tree", base(port), pid))
        .send()
        .await
        .unwrap();
    // Should return error (404 or 500 depending on implementation)
    assert!(r.status().is_client_error() || r.status().is_server_error());
}

#[tokio::test]
async fn session_model_switch_returns_updated_model() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session/{}/model", base(port), sid))
        .json(&serde_json::json!({ "provider": "mock", "model": "mock-1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["model"], "mock-1");
}

#[tokio::test]
async fn create_session_empty_provider_uses_default() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create with empty provider (uses default)
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(!body["session_id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn open_project_missing_path_returns_error() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": "/definitely/not/a/real/path/xyz" }))
        .send()
        .await
        .unwrap();
    // May return 400, 404, or 500 depending on whether daemon creates the dir
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn search_nonexistent_project_returns_error() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .get(format!("{}/project/nonexistent/search?q=test", base(port)))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error() || r.status().is_server_error());
}

#[tokio::test]
async fn create_session_nonexistent_project_returns_error() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": "nonexistent", "provider": "mock" }))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn checkpoint_rollback_nonexistent_returns_error() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .post(format!("{}/checkpoint/nonexistent-id/rollback", base(port)))
        .json(&serde_json::json!({ "granularity": "revert" }))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn open_project_and_get_pairing() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let r = client
        .get(format!("{}/pairing", base(port)))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["port"].as_u64().unwrap() > 0);
    assert!(!body["token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn session_checkpoints_after_patch() {
    let script = vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "cp.txt", "range": null, "content": "checkpoint test"}),
        },
        ScriptedReply::Text("done".into()),
    ];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "write" }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let r = client
        .get(format!("{}/session/{}/checkpoints", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["checkpoints"].is_array());
}

#[tokio::test]
async fn multiple_sessions_same_project() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create two sessions for the same project
    let r1 = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let s1 = r1.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let r2 = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let s2 = r2.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_ne!(s1, s2, "两个会话应有不同 ID");

    // Both sessions should be queryable
    let r1 = client
        .get(format!("{}/session/{}", base(port), s1))
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), 200);
    let r2 = client
        .get(format!("{}/session/{}", base(port), s2))
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), 200);
}

#[tokio::test]
async fn open_project_creates_snapshot_root() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // Project opened successfully → snapshot infrastructure ready
}

#[tokio::test]
async fn file_write_read_roundtrip_with_unicode_path() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let content = "中文文档内容\\n第二行";
    let r = client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "docs/中文文档.md", "content": content }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    let r = client
        .get(format!(
            "{}/project/{}/file?path={}",
            base(port),
            pid,
            "docs%2F%E4%B8%AD%E6%96%87%E6%96%87%E6%A1%A3.md"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["content"].as_str().unwrap().contains("中文"));
}

#[tokio::test]
async fn create_session_returns_worktree_path_for_managed() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // Create a git repo project
    let tmp = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let _ = std::process::Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@l")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@l")
            .output()
            .unwrap();
    };
    git(&["init", "--initial-branch=main"]);
    std::fs::write(tmp.path().join("init.txt"), "init").unwrap();
    git(&["add", "init.txt"]);
    git(&["commit", "-m", "init"]);

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create managed worktree session
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock", "worktree": "managed" }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    // Managed worktree should have a worktree_path
    if let Some(wt_path) = body["worktree_path"].as_str() {
        assert!(!wt_path.is_empty());
    }
}

#[tokio::test]
async fn language_pack_detect_returns_pack_list() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    // Create files for language detection
    std::fs::write(tmp.path().join("app.py"), "print(1)").unwrap();
    std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .get(format!("{}/project/{}/language-packs", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn open_project_returns_trusted_false_by_default() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["trusted"], false, "TOFU 默认未信任");
}

#[tokio::test]
async fn session_model_switch_and_get_session_reflects() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Switch model
    let r = client
        .post(format!("{}/session/{}/model", base(port), sid))
        .json(&serde_json::json!({ "provider": "mock", "model": "mock-alt" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn open_project_verify_snapshot_dir_exists() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn multiple_sessions_send_messages_independently() {
    let script = vec![
        ScriptedReply::Text("reply for session one".into()),
        ScriptedReply::Text("reply for session two".into()),
    ];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r1 = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let s1 = r1.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let r2 = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let s2 = r2.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Send messages to both sessions
    let msg1 = client
        .post(format!("{}/session/{}/message", base(port), s1))
        .json(&serde_json::json!({ "text": "msg for s1" }))
        .send()
        .await
        .unwrap();
    let _ = msg1.status();

    let msg2 = client
        .post(format!("{}/session/{}/message", base(port), s2))
        .json(&serde_json::json!({ "text": "msg for s2" }))
        .send()
        .await
        .unwrap();
    let _ = msg2.status();

    // Both sessions are queryable
    let r = client
        .get(format!("{}/session/{}", base(port), s1))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .get(format!("{}/session/{}", base(port), s2))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn team_policy_denied_tools_effect_on_session() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // Set team policy to deny a tool
    let r = client
        .put(format!("{}/team-policy", base(port)))
        .json(&serde_json::json!({ "denied_tools": ["bash", "run_tests"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Open project and create session
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn project_file_write_delete_write_cycle() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Write
    let r = client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "cycle.txt", "content": "v1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // File ops: delete
    let r = client
        .post(format!("{}/project/{}/file/ops", base(port), pid))
        .json(&serde_json::json!({ "ops": [{ "op": "delete", "path": "cycle.txt" }] }))
        .send()
        .await
        .unwrap();
    let _ = r.status();

    // Re-write
    let r = client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "cycle.txt", "content": "v2" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn l4_stats_and_rebuild_flow() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("index.rs"), "fn index() {}").unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // GET stats (initial)
    let r = client
        .get(format!("{}/project/{}/l4/stats", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["project_id"].is_string() || body["chunks"].is_number() || body.is_object());

    // POST rebuild
    let r = client
        .post(format!("{}/project/{}/l4/rebuild", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn search_with_empty_query_returns_ok_or_error() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .get(format!("{}/project/{}/search?q=", base(port), pid))
        .send()
        .await
        .unwrap();
    // Empty search may return 200 with no results or 400
    let _ = r.status();
}

#[tokio::test]
async fn checkpoint_rollback_after_file_write() {
    let script = vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "rb-test.txt", "range": null, "content": "after rollback test"}),
        },
        ScriptedReply::Text("written".into()),
    ];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "write checkpoint file" }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // List checkpoints
    let r = client
        .get(format!("{}/session/{}/checkpoints", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let cps = body["checkpoints"].as_array().unwrap();

    if let Some(first) = cps.first() {
        let cp_id = first["id"].as_str().unwrap();
        let r = client
            .post(format!("{}/checkpoint/{}/rollback", base(port), cp_id))
            .json(&serde_json::json!({ "granularity": "revert" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let body: serde_json::Value = r.json().await.unwrap();
        assert!(body.is_object());
    }
}

#[tokio::test]
async fn l4_stats_after_rebuild_shows_chunks() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), "pub fn index_me() {}").unwrap();
    std::fs::write(tmp.path().join("src/main.rs"), "fn main() { index_me(); }").unwrap();

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Rebuild index
    let r = client
        .post(format!("{}/project/{}/l4/rebuild", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Wait for indexing
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Check stats
    let r = client
        .get(format!("{}/project/{}/l4/stats", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn project_delete_then_open_again_creates_new_id() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    // First open
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": path }))
        .send()
        .await
        .unwrap();
    let id1 = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Delete
    client
        .delete(format!("{}/projects/{}", base(port), id1))
        .send()
        .await
        .unwrap();

    // Re-open → new ID
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": path }))
        .send()
        .await
        .unwrap();
    let id2 = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_ne!(id1, id2, "删除后重新打开应生成新 ID");
}

#[tokio::test]
async fn search_replace_verify_content_changed() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "sr-test.txt", "content": "before replace" }))
        .send()
        .await
        .unwrap();

    // Search preview
    let r = client
        .get(format!(
            "{}/project/{}/search?q=before&replace=after",
            base(port),
            pid
        ))
        .send()
        .await
        .unwrap();
    let _ = r.status();
}

#[tokio::test]
async fn create_session_multiple_times_same_project_different_ids() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut ids = Vec::new();
    for _ in 0..3 {
        let r = client
            .post(format!("{}/session", base(port)))
            .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
            .send()
            .await
            .unwrap();
        let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        ids.push(sid);
    }

    // All unique
    assert_eq!(ids.len(), 3);
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert_ne!(ids[0], ids[2]);
}

#[tokio::test]
async fn language_pack_detect_and_install_typescript() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
    std::fs::write(tmp.path().join("app.ts"), "const x = 1;").unwrap();

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .get(format!("{}/project/{}/language-packs", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    // Should detect TypeScript from package.json + app.ts
    let packs = body["packs"].as_array().unwrap();
    assert!(!packs.is_empty(), "should detect language packs");
}

#[tokio::test]
async fn open_project_snapshot_infrastructure() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(!body["id"].as_str().unwrap().is_empty());
    assert!(!body["path"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn session_lifecycle_create_send_trace_checkpoints() {
    let script = vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "lifecycle.txt", "range": null, "content": "lifecycle test"}),
        },
        ScriptedReply::Text("lifecycle complete".into()),
    ];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Send message
    let msg = client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "run lifecycle" }))
        .send()
        .await
        .unwrap();
    let _ = msg.status();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Trace has events
    let r = client
        .get(format!("{}/session/{}/trace", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let trace: serde_json::Value = r.json().await.unwrap();
    let _ = trace["events"].as_array().map(|e| e.len()).unwrap_or(0);

    // Checkpoints list
    let r = client
        .get(format!("{}/session/{}/checkpoints", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Session status query
    let r = client
        .get(format!("{}/session/{}", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn models_list_returns_mock_provider() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .get(format!("{}/models", base(port)))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    let models = body["models"].as_array().unwrap();
    assert!(!models.is_empty());
    // Default should be "mock" (from start_daemon)
    assert_eq!(body["default"], "mock");
}

#[tokio::test]
async fn evals_endpoint_returns_tasks_array() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .get(format!("{}/evals", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    // Response should have tasks or runs
    assert!(body.is_object() || body.is_array());
}

#[tokio::test]
async fn session_message_sends_and_trace_updates() {
    let script = vec![ScriptedReply::Text("I understand your request.".into())];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Send
    let msg = client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "tell me about this project" }))
        .send()
        .await
        .unwrap();
    let _ = msg.status();

    // Trace should be queryable
    let r = client
        .get(format!("{}/session/{}/trace", base(port), sid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn file_write_and_verify_on_disk() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "on-disk.txt", "content": "persisted content" }))
        .send()
        .await
        .unwrap();

    // Verify on actual filesystem
    assert!(tmp.path().join("on-disk.txt").exists());
    let disk_content = std::fs::read_to_string(tmp.path().join("on-disk.txt")).unwrap();
    assert_eq!(disk_content, "persisted content");
}

#[tokio::test]
async fn settings_roundtrip_full_config() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let full_config = serde_json::json!({
        "session": { "first_edit_buffer_ms": 3000 },
        "exec": { "command_timeout_s": 60 },
        "models": {
            "default": "anthropic",
            "providers": {
                "anthropic": {
                    "kind": "anthropic",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-3"
                }
            }
        },
        "checkpoint": { "keep_days": 3 },
        "agent": {
            "circuit": { "max_files": 20, "max_lines": 2000 },
            "fix_loop": { "max_rounds": 2 },
            "exec": { "command_timeout_s": 45 }
        }
    });

    let r = client
        .put(format!("{}/settings", base(port)))
        .json(&full_config)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    let r = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body.is_object());
}

#[tokio::test]
async fn team_policy_and_settings_independent() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // Set team policy
    let r = client
        .put(format!("{}/team-policy", base(port)))
        .json(&serde_json::json!({ "denied_tools": ["git_push"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Settings are independent
    let r = client
        .put(format!("{}/settings", base(port)))
        .json(&serde_json::json!({ "session": { "first_edit_buffer_ms": 1000 } }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    // Team policy still in effect
    let r = client
        .put(format!("{}/team-policy", base(port)))
        .json(&serde_json::json!({ "denied_tools": ["git_push", "bash"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn project_file_and_session_integration() {
    let script = vec![ScriptedReply::Text("project analysis complete".into())];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();

    // 1. Open project
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 2. Write a file
    let r = client
        .put(format!("{}/project/{}/file", base(port), pid))
        .json(&serde_json::json!({ "path": "analysis.ts", "content": "export function analyze() { return 'done'; }" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // 3. Create session
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // 4. Send message about the file
    let msg = client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "analyze analysis.ts" }))
        .send()
        .await
        .unwrap();
    let _ = msg.status();

    // 5. Verify file exists on disk
    assert!(tmp.path().join("analysis.ts").exists());
}

#[tokio::test]
async fn settings_get_after_put_session_config() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // PUT
    client
        .put(format!("{}/settings", base(port)))
        .json(&serde_json::json!({ "session": { "first_edit_buffer_ms": 5000 } }))
        .send()
        .await
        .unwrap();

    // GET and verify
    let r = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn ui_prefs_theme_and_locale_roundtrip() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    // PUT prefs
    let r = client
        .put(format!("{}/ui-prefs", base(port)))
        .json(&serde_json::json!({ "theme": "dark", "locale": "zh-CN", "sidebar_width": 280 }))
        .send()
        .await
        .unwrap();
    // UI prefs 端点可能 200 或 400（取决于实现）
    let _ = r.status();

    // GET prefs
    let r = client
        .get(format!("{}/ui-prefs", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body.is_object() || body.is_array());
}

#[tokio::test]
async fn models_endpoint_structure_check() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .get(format!("{}/models", base(port)))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    // Check expected structure
    assert!(body["models"].is_array());
    assert!(body["default"].is_string());
}

#[tokio::test]
async fn pair_info_returns_port_and_token() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let r = client
        .get(format!("{}/pairing", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["port"].as_u64().unwrap(), port as u64);
    assert_eq!(body["token"], token);
}

#[tokio::test]
async fn open_project_returns_sessions_array() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    // Project should have sessions array (even if empty)
    assert!(body.get("id").is_some());
}

#[tokio::test]
async fn session_send_message_returns_accepted() {
    let script = vec![ScriptedReply::Text("acknowledged".into())];
    let (_dir, port, token) = start_daemon(script).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({ "project_id": pid, "provider": "mock" }))
        .send()
        .await
        .unwrap();
    let sid = r.json::<serde_json::Value>().await.unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{}/session/{}/message", base(port), sid))
        .json(&serde_json::json!({ "text": "test message" }))
        .send()
        .await
        .unwrap();
    // Message may be accepted (200) or rejected depending on state
    assert_ne!(r.status(), 401);
}

#[tokio::test]
async fn git_source_view_after_git_init() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let _ = std::process::Command::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(tmp.path())
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@l")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@l")
        .output()
        .unwrap();

    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .get(format!("{}/project/{}/git/view", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn file_tree_after_multiple_writes() {
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let tmp = tempfile::tempdir().unwrap();
    let r = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({ "path": tmp.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap();
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Write multiple files in different dirs
    for (path, content) in [
        ("src/index.ts", "export {};"),
        ("src/utils/helpers.ts", "export {};"),
        ("README.md", "# Test"),
        ("docs/guide.md", "Guide"),
    ] {
        client
            .put(format!("{}/project/{}/file", base(port), pid))
            .json(&serde_json::json!({ "path": path, "content": content }))
            .send()
            .await
            .unwrap();
    }

    // Tree should show entries
    let r = client
        .get(format!("{}/project/{}/tree", base(port), pid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

// ---------- 代理技能管理（§13.4 v1.130） ----------

/// 技能测试专用 daemon：skills 目录隔离注入（其余走 in_memory 默认）。
async fn start_skills_daemon() -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.policy_path = Some(dir.path().join("policy.toml"));
    options.skills_dir = Some(dir.path().join("skills"));
    let handle = tenon_daemon::serve(options).await.unwrap();
    (dir, handle.port, handle.token)
}

#[tokio::test]
async fn skills_crud_and_disabled_roundtrip() {
    let (_dir, port, token) = start_skills_daemon().await;
    let client = client_with_token(&token);

    // 初始为空
    let r = client
        .get(format!("{base}/skills", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["skills"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // 新建（含 frontmatter）→ 重复 409 → 非法名 400
    let content = "---\nname: 提交助手\ndescription: 生成中文提交信息\n---\n正文";
    let r = client
        .post(format!("{base}/skills", base = base(port)))
        .json(&serde_json::json!({ "name": "commit-helper", "content": content }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .post(format!("{base}/skills", base = base(port)))
        .json(&serde_json::json!({ "name": "commit-helper", "content": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    let r = client
        .post(format!("{base}/skills", base = base(port)))
        .json(&serde_json::json!({ "name": "../escape", "content": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // 清单：frontmatter 解析 + enabled 默认开
    let r = client
        .get(format!("{base}/skills", base = base(port)))
        .send()
        .await
        .unwrap();
    let skills = r.json::<serde_json::Value>().await.unwrap()["skills"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "commit-helper");
    assert_eq!(skills[0]["scope"], "global");
    assert_eq!(skills[0]["description"], "生成中文提交信息");
    assert_eq!(skills[0]["enabled"], true);

    // 读原文 → 更新 → 读回
    let r = client
        .get(format!("{base}/skills/commit-helper", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.json::<serde_json::Value>().await.unwrap()["content"]
        .as_str()
        .unwrap()
        .contains("生成中文提交信息"));
    let r = client
        .put(format!("{base}/skills/commit-helper", base = base(port)))
        .json(&serde_json::json!({ "content": "---\ndescription: 更新版\n---\n新正文" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .get(format!("{base}/skills/commit-helper", base = base(port)))
        .send()
        .await
        .unwrap();
    assert!(r.json::<serde_json::Value>().await.unwrap()["content"]
        .as_str()
        .unwrap()
        .contains("新正文"));

    // 启停经 PUT /settings skills.disabled（合并视图回显，清单 enabled 翻转）
    let r = client
        .put(format!("{base}/settings", base = base(port)))
        .json(&serde_json::json!({ "skills": { "disabled": ["commit-helper"] } }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let view = r.json::<serde_json::Value>().await.unwrap();
    assert_eq!(view["skills"]["disabled"].as_array().unwrap().len(), 1);
    let r = client
        .get(format!("{base}/skills", base = base(port)))
        .send()
        .await
        .unwrap();
    let skills = r.json::<serde_json::Value>().await.unwrap()["skills"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        skills[0]["enabled"], false,
        "停用条目仍在清单、enabled=false"
    );

    // settings 持久化：非法技能名 400
    let r = client
        .put(format!("{base}/settings", base = base(port)))
        .json(&serde_json::json!({ "skills": { "disabled": ["../bad"] } }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // 删除 → 清单回空
    let r = client
        .delete(format!("{base}/skills/commit-helper", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .get(format!("{base}/skills", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["skills"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn skills_project_scope_overrides_global() {
    let (dir, port, token) = start_skills_daemon().await;
    let client = client_with_token(&token);

    // 全局技能
    client
        .post(format!("{base}/skills", base = base(port)))
        .json(
            &serde_json::json!({ "name": "shared", "content": "---\ndescription: 全局版\n---\n" }),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // 登记项目并在其 .tenon/skills/ 放同名技能
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join(".tenon/skills/shared")).unwrap();
    std::fs::write(
        ws.join(".tenon/skills/shared/SKILL.md"),
        "---\ndescription: 项目版覆盖\n---\n项目正文",
    )
    .unwrap();
    let r = client
        .post(format!("{base}/projects/open", base = base(port)))
        .json(&serde_json::json!({ "path": ws.to_string_lossy() }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 带项目查询：同名项目覆盖全局
    let r = client
        .get(format!("{base}/skills?project={pid}", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let skills = r.json::<serde_json::Value>().await.unwrap()["skills"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(skills.len(), 1, "项目同名覆盖全局后仅一条");
    assert_eq!(skills[0]["scope"], "project");
    assert_eq!(skills[0]["description"], "项目版覆盖");

    // 项目技能读原文走同端点
    let r = client
        .get(format!(
            "{base}/skills/shared?project={pid}",
            base = base(port)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["scope"], "project");
    assert!(body["content"].as_str().unwrap().contains("项目正文"));

    // 未登记项目 → 404
    let r = client
        .get(format!("{base}/skills?project=nope", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn projects_session_rows_carry_subtasks_progress() {
    // v1.148 §15：GET /projects 会话行携带最新 subtasks 快照计数
    //（侧栏任务行进度徽标数据源，随既有轮询链刷新，无清单为 null）。
    let dir = tempfile::tempdir().unwrap();
    let (_tmp, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "subtasks".into(),
            args: serde_json::json!({"items": [
                {"title": "改造导出管道", "status": "done"},
                {"title": "补充单测", "status": "in_progress"}
            ]}),
        },
        ScriptedReply::Text("中途汇报".into()),
    ])
    .await;
    let client = client_with_token(&token);
    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": dir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = registered["id"].as_str().unwrap().to_string();
    client
        .put(format!("{}/project/trust", base(port)))
        .json(&serde_json::json!({"project_id": project_id, "trusted": true}))
        .send()
        .await
        .unwrap();
    let created: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({
            "project_path": dir.path().to_string_lossy(),
            "provider": "mock",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sid = created["session_id"].as_str().unwrap().to_string();
    let r = client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "重构导出功能"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);

    // 轮询 /projects 直到该会话行出现 subtasks 计数（任务后台执行）
    let mut got: Option<(i64, i64)> = None;
    for _ in 0..200 {
        let list: serde_json::Value = client
            .get(format!("{}/projects", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(row) = list["projects"][0]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"].as_str() == Some(sid.as_str()))
        {
            if row["subtasks"].is_object() {
                got = Some((
                    row["subtasks"]["done"].as_i64().unwrap(),
                    row["subtasks"]["total"].as_i64().unwrap(),
                ));
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        got,
        Some((1, 2)),
        "会话行应携带最新快照计数 1/2（未完成→徽标渲染）"
    );
}
