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
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
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

    // file ops
    client
        .post(format!("{}/project/{pid}/file/ops", base(port)))
        .json(&serde_json::json!({"ops": [{"op": "create_file", "path": "new.rs", "content": "fn a() {}\n"}]}))
        .send()
        .await
        .unwrap();
    assert!(project.join("new.rs").exists());

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
    assert!(list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["open"].as_bool().unwrap()));

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

    // 关闭只摘除 runtime，不删除登记或磁盘。
    let closed: serde_json::Value = client
        .post(format!("{}/projects/{bi}/close", base(port)))
        .json(&serde_json::json!({"mode": "force"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(closed["closed"], true);
    assert!(beta.join("root.txt").exists());

    // 关闭后文件 API 不得继续访问该 ProjectRuntime。
    let closed_file = client
        .get(format!("{}/project/{bi}/file?path=root.txt", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(closed_file.status(), 404);
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
    eprintln!("PLUGIN first: {first}");
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

    // 2. 安装第一调：权限 diff + D 级卡（新增 net:registry:npm 高亮）
    let first: serde_json::Value = local
        .post(format!("{}/plugins/install", base(handle.port)))
        .json(&serde_json::json!({
            "entry": search["hits"][0],
            "installed_permissions": ["fs.read:project"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    eprintln!("PLUGIN first: {first}");
    assert_eq!(first["level"], "d");
    assert_eq!(
        first["permission_diff"]["added"][0], "net:registry:npm",
        "新增权限高亮（§13.2）: {first}"
    );
    let approval_id = first["approval_id"].as_str().unwrap().to_string();

    // 3. 未批准复调 → 403
    let denied = local
        .post(format!("{}/plugins/install", base(handle.port)))
        .json(&serde_json::json!({
            "entry": search["hits"][0],
            "approval_id": approval_id,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // 4. 批准 → 安装 + 入库
    local
        .post(format!("{}/approval/{approval_id}", base(handle.port)))
        .json(&serde_json::json!({"decision": "once"}))
        .send()
        .await
        .unwrap();
    let installed: serde_json::Value = local
        .post(format!("{}/plugins/install", base(handle.port)))
        .json(&serde_json::json!({
            "entry": search["hits"][0],
            "approval_id": approval_id,
            "installed_permissions": ["fs.read:project"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(installed["installed"], true);
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

    // 关闭后文件 API 拒绝，ProjectRuntime watcher / 资源被释放。
    let closed: serde_json::Value = client
        .post(format!("{}/projects/{project_id}/close", base(port)))
        .json(&serde_json::json!({"mode": "force"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(closed["closed"], true);
    let denied = client
        .get(format!(
            "{}/project/{project_id}/file?path=watched.txt",
            base(port)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);
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
        "session": {"mode": "auto", "first_edit_buffer_ms": 1500, "approval_timeout_s": 60},
        "exec": {"command_timeout_s": 90}
    }))
    .await;
    assert_eq!(r.status(), 200);
    let merged: serde_json::Value = r.json().await.unwrap();
    assert_eq!(merged["session"]["mode"], "auto");
    assert_eq!(merged["session"]["first_edit_buffer_ms"], 1500);
    assert_eq!(merged["exec"]["command_timeout_s"], 90);

    // 持久化文件（0600）
    let file = tenon_config::Config::data_dir().join("settings.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["session"]["mode"], "auto");

    // 非法值逐一 400
    for bad in [
        serde_json::json!({"session": {"mode": "yolo"}}),
        serde_json::json!({"session": {"first_edit_buffer_ms": -1}}),
        serde_json::json!({"session": {"approval_timeout_s": 1}}),
        serde_json::json!({"exec": {"command_timeout_s": 99999}}),
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
