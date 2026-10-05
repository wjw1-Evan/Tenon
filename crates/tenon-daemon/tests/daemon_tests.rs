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
            .get(format!("{}/project", base(port)))
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

    // v1.15 兼容层：legacy 隐式项目必须显式 project_id。
    let legacy = client
        .get(format!("{}/file?path=a.txt", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.status(), 409);

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

    // 同名相对路径的脏缓冲按项目隔离。
    for (project_id, content) in [(ai, "alpha dirty"), (bi, "beta dirty")] {
        let resp = client
            .put(format!("{}/project/{project_id}/buffers", base(port)))
            .json(&serde_json::json!({"path": "buffer.txt", "dirty": content}))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
    }
    let a_buffers: serde_json::Value = client
        .get(format!("{}/project/{ai}/buffers", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b_buffers: serde_json::Value = client
        .get(format!("{}/project/{bi}/buffers", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        a_buffers["paths"].as_array().unwrap().len(),
        b_buffers["paths"].as_array().unwrap().len()
    );
    client
        .delete(format!(
            "{}/project/{ai}/buffers?path=buffer.txt",
            base(port)
        ))
        .send()
        .await
        .unwrap();
    let a_after: serde_json::Value = client
        .get(format!("{}/project/{ai}/buffers", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(a_after["paths"].as_array().unwrap().is_empty());
    let b_after: serde_json::Value = client
        .get(format!("{}/project/{bi}/buffers", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(b_after["paths"].as_array().unwrap().len(), 1);

    // 嵌套根默认拒绝。
    let nested_root = beta.join("nested-root");
    std::fs::create_dir_all(&nested_root).unwrap();
    let nested = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": nested_root.to_string_lossy()}))
        .send()
        .await
        .unwrap();
    assert_eq!(nested.status(), 409, "嵌套项目根默认拒绝");

    // v1.60 登记即用：无显式关闭；移除登记时 daemon 自行摘除 runtime。
    assert!(beta.join("root.txt").exists());

    // 移除登记只删除注册记录；磁盘内容与仍打开的 A runtime 不受影响。
    let removed = client
        .delete(format!("{}/projects/{bi}", base(port)))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(removed["removed"], true);
    assert_eq!(removed["disk_contents_deleted"], false);
    assert!(beta.join("root.txt").exists());
    let list_after_remove: serde_json::Value = client
        .get(format!("{}/projects", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list_after_remove["projects"].as_array().unwrap().len(), 1);
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
        .put(format!("{}/project", base(port)))
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

    let search: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/l4/search?q=login%20authenticate&k=5",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let hits = search["hits"].as_array().unwrap();
    assert!(!hits.is_empty(), "{search}");
    assert_eq!(hits[0]["path"], "src/auth.rs");

    // watcher 增量：修改后旧 token 不应再命中，新 token 应命中。
    std::fs::write(
        project.join("src/auth.rs"),
        "pub fn render_canvas_and_pixels\n",
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let old_search: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/l4/search?q=login%20authenticate&k=5",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let new_search: serde_json::Value = client
        .get(format!(
            "{}/project/{pid}/l4/search?q=render%20canvas&k=5",
            base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
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

    let old_score = old_search["hits"]
        .as_array()
        .unwrap()
        .first()
        .and_then(|hit| hit["score"].as_f64())
        .unwrap_or(-1.0);
    let new_score = new_search["hits"]
        .as_array()
        .unwrap()
        .first()
        .and_then(|hit| hit["score"].as_f64())
        .unwrap_or(-1.0);
    assert!(
        new_score > old_score,
        "new query should outrank stale query: old={old_search} new={new_search}"
    );
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

    let default: serde_json::Value = client
        .get(format!("{}/team-policy", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(default["force_interactive"], false);
    assert_eq!(default["denied_tools"].as_array().unwrap().len(), 0);

    let saved = client
        .put(format!("{}/team-policy", base(port)))
        .json(&serde_json::json!({
            "force_interactive": true,
            "denied_tools": ["git_push", "apply_patch", "apply_patch"],
            "max_cost_usd": 0.25,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let saved: serde_json::Value = saved.json().await.unwrap();
    assert_eq!(saved["force_interactive"], true);
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
    assert_eq!(settings["team_policy"]["force_interactive"], true);
    assert_eq!(settings["team_policy"]["max_cost_usd"], 0.25);

    let policy_path = tmp.path().join("policy.toml");
    let persisted = std::fs::read_to_string(&policy_path).unwrap();
    assert!(persisted.contains("force_interactive = true"));
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
        serde_json::json!({"force_interactive": "yes"}),
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
        .get(format!("{}/team-policy", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged["force_interactive"], true);
    assert_eq!(unchanged["max_cost_usd"], 0.25);
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
        .put(format!("{}/project", base(port)))
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
        .get(format!("{}/project", base(port)))
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

#[tokio::test]
async fn plugin_registry_install_flow_with_permission_diff() {
    // registry fixture：静态 index + 包体（同一本地 HTTP 服务器）
    let manifest_yaml = r#"id: community.demo
version: 1.0.0
runtime: external
permissions:
  - fs.read:project
  - net:registry:npm
provides:
  languages: [demo]
signature: ""
"#;
    let payload = manifest_yaml.as_bytes().to_vec();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let pkg_port = listener.local_addr().unwrap().port();
    let index = serde_json::json!({
        "plugins": [{
            "id": "community.demo", "version": "1.0.0",
            "sha256": tenon_registry::sha256_hex(&payload),
            "signature": "", "url": format!("http://127.0.0.1:{pkg_port}/pkg.yaml"),
            "description": "demo plugin"
        }]
    });
    let index_body = index.to_string();
    std::thread::spawn(move || {
        // 直接使用移动进来的 listener（绑定已在上方完成，避免重绑 AddrInUse）
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut buf = [0u8; 4096];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let req = String::from_utf8_lossy(&buf);
            let body = if req.contains("pkg.yaml") {
                payload.clone()
            } else {
                index_body.clone().into_bytes()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = std::io::Write::write_all(&mut s, resp.as_bytes());
            let _ = std::io::Write::write_all(&mut s, &body);
            let _ = std::io::Write::flush(&mut s);
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("ok".into())],
    ))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    let handle = tenon_daemon::serve(options).await.unwrap();
    let local = client_with_token(&handle.token);

    // 1. registry 检索
    let search: serde_json::Value = local
        .put(format!("{}/plugins", base(handle.port)))
        .json(&serde_json::json!({"query": "demo", "registry_url": format!("http://127.0.0.1:{pkg_port}/index.json")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(search["hits"][0]["id"], "community.demo");

    // 2. 校验通过后直执安装；响应保留权限 diff 供审计展示
    let installed: serde_json::Value = local
        .post(format!("{}/plugins/install", base(handle.port)))
        .json(&serde_json::json!({
            "entry": search["hits"][0],
            "installed_permissions": ["fs.read:project"],
            // 测试主机可能装有 ~/.tenon signing key；本地社区 fixture 显式走无钥开发模式。
            "public_key": "0".repeat(64),
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(installed["installed"], true);
    assert_eq!(installed["permission_diff"]["added"][0], "net:registry:npm");
    let list: serde_json::Value = local
        .get(format!("{}/plugins", base(handle.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["installed"][0]["id"], "community.demo");
}

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
async fn portfolio_task_orchestrates_project_scoped_children() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = dir.path().join("alpha");
    let beta = dir.path().join("beta");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();

    let (_tmp, port, token) = start_daemon(vec![
        ScriptedReply::Text("alpha done".into()),
        ScriptedReply::Text("beta done".into()),
    ])
    .await;
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
                .json::<serde_json::Value>()
                .await
                .unwrap()
        }
    };
    let a = open(&alpha).await;
    let b = open(&beta).await;

    let created: serde_json::Value = client
        .post(format!("{}/portfolio-tasks", base(port)))
        .json(&serde_json::json!({
            "title": "two projects",
            "provider": "mock",
            "children": [
                {"project_id": a["id"], "text": "alpha task"},
                {"project_id": b["id"], "text": "beta task"}
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["children"].as_array().unwrap().len(), 2);
    assert_ne!(
        created["children"][0]["session_id"],
        created["children"][1]["session_id"]
    );

    for _ in 0..100 {
        let tasks: serde_json::Value = client
            .get(format!("{}/portfolio-tasks", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let task = &tasks["tasks"][0];
        if task["status"] == "done" {
            assert!(task["children"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["status"] == "done"));
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("组合任务应在预算内完成");
}

#[tokio::test]
async fn project_runtime_streams_scoped_file_changes() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("watched");
    std::fs::create_dir_all(&project).unwrap();

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
        "session": {"mode": "auto", "first_edit_buffer_ms": 1500},
        "exec": {"command_timeout_s": 90},
        "privacy": {"telemetry": true, "crash_reports": "opt_in"},
        "update": {"channel": "auto"}
    }))
    .await;
    assert_eq!(r.status(), 200);
    let merged: serde_json::Value = r.json().await.unwrap();
    assert_eq!(merged["session"]["mode"], "auto");
    assert_eq!(merged["session"]["first_edit_buffer_ms"], 1500);
    assert_eq!(merged["exec"]["command_timeout_s"], 90);
    assert_eq!(merged["privacy"]["telemetry"], true);
    assert_eq!(merged["privacy"]["crash_reports"], "opt_in");
    assert_eq!(merged["update"]["channel"], "auto");

    // 持久化文件（0600）
    let file = _tmp.path().join("settings.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["session"]["mode"], "auto");
    assert_eq!(v["privacy"]["telemetry"], true);
    assert_eq!(v["privacy"]["crash_reports"], "opt_in");
    assert_eq!(v["update"]["channel"], "auto");

    // 非法值逐一 400
    for bad in [
        serde_json::json!({"session": {"mode": "yolo"}}),
        serde_json::json!({"session": {"first_edit_buffer_ms": -1}}),
        serde_json::json!({"exec": {"command_timeout_s": 99999}}),
        serde_json::json!({"privacy": {"crash_reports": "always"}}),
        serde_json::json!({"update": {"channel": "daily"}}),
    ] {
        let r = put(bad.clone()).await;
        assert_eq!(r.status(), 400, "bad={bad}");
    }

    // 新会话默认档：项目未信任时 auto 回退交互档（§12.7）
    let dir = tempfile::tempdir().unwrap();
    let p: serde_json::Value = client
        .put(format!("{}/project", base(port)))
        .json(&serde_json::json!({"path": dir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let s: serde_json::Value = client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({"project_id": p["id"], "mode": ""}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        s["session_id"].is_string(),
        "未信任项目 auto 应回退交互档建会话"
    );
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

    // PUT /project 登记时带显示名 → 摘要返回自定义名
    let registered: serde_json::Value = client
        .put(format!("{url}/project"))
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
        .put(format!("{url}/project"))
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
    let sha = tenon_laya::sha256_hex(&model);
    let sk = laya_test_signer();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let manifest = serde_json::json!({
        "laya": {
            "version": 1,
            "sha256": sha,
            "signature": hex::encode(sk.sign(sha.as_bytes()).to_bytes()),
            "url": format!("http://{addr}/model.json"),
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
    assert!(
        models["laya"]["version"].as_array().is_some(),
        "装载后应有模型版本：{models}"
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
async fn laya_manual_download_installs_without_approval() {
    // v1.71：/models/laya/download 去审批化——直接下载安装，不再两阶段 D 卡
    let (registry, _public_key) = spawn_laya_registry().await;
    let (_dir, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);

    let r = client
        .post(format!("{}/models/laya/download", base(port)))
        .json(&serde_json::json!({ "registry_url": registry }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["installed"], serde_json::json!(true));

    let models: serde_json::Value = client
        .get(format!("{}/models", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["laya"]["downloaded"], serde_json::json!(true));
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
