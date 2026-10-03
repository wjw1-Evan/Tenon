//! daemon 端到端集成测试（真 HTTP/WS；§15 API 全链路 + §12.6 鉴权面）。

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
    let handle = serve(options).await.unwrap();
    (dir, handle.port, handle.token)
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
        .put(format!("{}/project", base(port)))
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
async fn approval_flow_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("f.txt"), "x\n").unwrap();

    let (_tmp, port, token) = start_daemon(vec![
        ScriptedReply::Tool {
            name: "http_fetch".into(),
            args: serde_json::json!({"url": "https://example.com/changelog"}),
        },
        ScriptedReply::Text("已拒绝，不再重试".into()),
    ])
    .await;
    let client = client_with_token(&token);

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

    client
        .post(format!("{}/session/{sid}/message", base(port)))
        .json(&serde_json::json!({"text": "抓取 changelog"}))
        .send()
        .await
        .unwrap();

    // 等待审批卡出现（C 级恒审批，域名明示）
    let mut approval_id = None;
    for _ in 0..100 {
        let trace: serde_json::Value = client
            .get(format!("{}/session/{sid}/trace", base(port)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        for ev in trace["events"].as_array().unwrap() {
            if ev["type"] == "approval_request" {
                assert_eq!(ev["payload"]["level"], "c");
                assert!(
                    ev["payload"]["summary"]
                        .as_str()
                        .unwrap()
                        .contains("example.com"),
                    "C 级卡必须明示域名（§12.2）"
                );
                approval_id = Some(ev["payload"]["approval_id"].as_str().unwrap().to_string());
            }
        }
        if approval_id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let approval_id = approval_id.expect("应出现 C 级审批卡");

    // 拒绝 → 任务以回答收尾
    client
        .post(format!("{}/approval/{approval_id}", base(port)))
        .json(&serde_json::json!({"decision": "deny"}))
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
        if status["status"] == "done" && status["outcome"].is_object() {
            assert!(status["outcome"]["Done"]["answer"]
                .as_str()
                .unwrap()
                .contains("已拒绝"));
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("任务应在拒绝后完成");
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
    let sid = {
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
        .get(format!("{}/project/{sid}/tree", base(port)))
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
        .get(format!("{}/file?path=a.txt", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read["content"], "alpha\nbeta\n");
    client
        .put(format!("{}/file", base(port)))
        .json(&serde_json::json!({"path": "a.txt", "content": "changed\n"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "changed\n"
    );

    // 路径越界被拒
    let escape = client
        .get(format!("{}/file?path=../etc/passwd", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(escape.status(), 400, "写守卫：越界读取拒绝");

    // 搜索
    let hits: serde_json::Value = client
        .get(format!("{}/search?q=beta", base(port)))
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
        .get(format!("{}/search?q=beta&replace=BETA", base(port)))
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

    // file ops
    client
        .post(format!("{}/file/ops", base(port)))
        .json(&serde_json::json!({"ops": [{"op": "create_file", "path": "new.rs", "content": "fn a() {}\n"}]}))
        .send()
        .await
        .unwrap();
    assert!(project.join("new.rs").exists());
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
        .put(format!("{}/project", base(port)))
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

    // 两阶段 D 级审批：第一调出卡
    let first: serde_json::Value = client
        .post(format!(
            "{}/project/{pid}/language-packs/install",
            base(port)
        ))
        .json(&serde_json::json!({"pack": "typescript"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["level"], "d");
    let approval_id = first["approval_id"].as_str().unwrap().to_string();

    // 未批准直接复调 → 403
    let denied = client
        .post(format!(
            "{}/project/{pid}/language-packs/install",
            base(port)
        ))
        .json(&serde_json::json!({"pack": "typescript", "approval_id": approval_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403, "D 级审批未通过不得安装");
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
    let handle = tenon_daemon::serve(options).await.unwrap();
    let client = client_with_token(&handle.token);

    // 配对入口：免 token 可达（本机），含 ws_ticket 与 lan 状态
    let pairing: serde_json::Value = reqwest::get(format!("{}/pairing", base(handle.port)))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pairing["local_only"], true);
    assert_eq!(pairing["lan_enabled"], false);
    assert!(pairing["ws_ticket"].as_str().is_some());

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
