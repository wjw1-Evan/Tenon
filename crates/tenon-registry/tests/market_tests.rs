//! 市场客户端测试（§13.5 v1.145）：条目校验、镜像链清单拉取（jsDelivr→raw 回退）、
//! 技能目录列举与文件下载（上限）、sidecar 落盘与更新覆盖——经本地 mock HTTP。

use std::io::{Read, Write};
use tenon_registry::market::{
    self, install_skill_dir, read_sidecar, MarketClient, MarketEntry, MarketKind, MarketSidecar,
};

/// 启动本地 mock HTTP：按路径子串路由（先匹配者优先）。
fn spawn_mock(routes: Vec<(&str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let owned: Vec<(String, String)> = routes
        .into_iter()
        .map(|(p, b)| (p.to_string(), b))
        .collect();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let hit = owned.iter().find(|(p, _)| path.contains(p.as_str()));
            let (status, body) = match hit {
                Some((_, b)) => ("200 OK", b.clone()),
                None => ("404 Not Found", "not found".to_string()),
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).ok();
        }
    });
    format!("http://{addr}")
}

fn skill_entry() -> MarketEntry {
    serde_json::from_value(serde_json::json!({
        "kind": "skill", "name": "pdf", "description": "PDF 文档处理",
        "path": "skills/pdf", "version": "1.2.0"
    }))
    .unwrap()
}

fn mcp_entry() -> MarketEntry {
    serde_json::from_value(serde_json::json!({
        "kind": "mcp", "name": "github", "description": "GitHub API",
        "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
        "permissions": ["net:*"], "version": "1.0.0"
    }))
    .unwrap()
}

#[test]
fn skill_entry_validation() {
    assert!(skill_entry().validate("owner/market").is_ok());
    // source 缺省回退市场源；显式 source 非法形态拒绝。
    let mut e = skill_entry();
    e.source = Some("bad source".into());
    assert!(e.validate("owner/market").is_err());
    // path 缺失 / 穿越拒绝。
    let mut e = skill_entry();
    e.path = None;
    assert!(e.validate("owner/market").is_err());
    let mut e = skill_entry();
    e.path = Some("../escape".into());
    assert!(e.validate("owner/market").is_err());
    // 技能名非法。
    let mut e = skill_entry();
    e.name = "../escape".into();
    assert!(e.validate("owner/market").is_err());
    // 市场源形态非法。
    assert!(skill_entry().validate("no-slash").is_err());
}

#[test]
fn mcp_entry_validation() {
    assert!(mcp_entry().validate("owner/market").is_ok());
    // 服务器名禁下划线。
    let mut e = mcp_entry();
    e.name = "my_server".into();
    assert!(e.validate("owner/market").is_err());
    // 启动器白名单。
    let mut e = mcp_entry();
    e.command = Some("/bin/sh".into());
    assert!(e.validate("owner/market").is_err());
    // args 控制字符拒绝；npm 包名 / 合法。
    let mut e = mcp_entry();
    e.args = vec!["-y".into(), "pkg".into()];
    assert!(e.validate("owner/market").is_ok(), "合法 args 应通过");
    let mut e = mcp_entry();
    e.args = vec!["bad\narg".into()];
    assert!(e.validate("owner/market").is_err());
    // env 仅允许 env:VAR 引用。
    let mut e = mcp_entry();
    e.env.insert("TOKEN".into(), "ghp_plain".into());
    assert!(e.validate("owner/market").is_err());
    let mut e = mcp_entry();
    e.env.insert("TOKEN".into(), "env:MY_TOKEN".into());
    assert!(e.validate("owner/market").is_ok());
    // permissions 仅 net:*。
    let mut e = mcp_entry();
    e.permissions = vec!["fs.write:/".into()];
    assert!(e.validate("owner/market").is_err());
}

#[tokio::test]
async fn fetch_manifest_prefers_cdn_falls_back_to_raw() {
    let manifest = serde_json::json!({
        "name": "示例市场",
        "entries": [skill_entry(), mcp_entry()]
    })
    .to_string();
    // cdn（路径含 @main）404 → raw（路径含 /main/）回退。
    let base = spawn_mock(vec![("/main/marketplace.json", manifest.clone())]);
    let client = MarketClient::with_bases(&format!("{base}/cdn"), &base, &base, &base);
    let fetched = client.fetch_manifest("mocksrc/repo", "main").await.unwrap();
    assert_eq!(fetched.entries.len(), 2);
    assert_eq!(fetched.entries[0].kind, MarketKind::Skill);
    assert_eq!(
        fetched.entries[1].resolved_source("mocksrc/repo"),
        "mocksrc/repo"
    );

    // cdn 可用即止（raw 不注册）。
    let base = spawn_mock(vec![("@main/marketplace.json", manifest.clone())]);
    let client = MarketClient::with_bases(&base, &format!("{base}/raw-404"), &base, &base);
    let fetched = client.fetch_manifest("mocksrc/repo", "main").await.unwrap();
    assert_eq!(fetched.entries.len(), 2);
}

#[tokio::test]
async fn fetch_manifest_all_mirrors_down_errors() {
    let base = spawn_mock(vec![]);
    let client = MarketClient::with_bases(&base, &base, &base, &base);
    let err = client
        .fetch_manifest("mocksrc/repo", "main")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("不可达"));
}

#[tokio::test]
async fn fetch_skill_files_lists_downloads_and_enforces_caps() {
    // data API 列举两个文件；cdn 提供内容（@main 路径优先命中）。
    let listing = serde_json::json!({
        "files": [
            { "name": "/skills/pdf/SKILL.md", "size": 11, "type": "file" },
            { "name": "/skills/pdf/scripts/run.py", "size": 4, "type": "file" }
        ]
    })
    .to_string();
    let skill_md = "# PDF 技能".as_bytes().to_vec();
    let routes = vec![
        ("/v1/packages/gh/", listing),
        (
            "@main/skills/pdf/SKILL.md",
            String::from_utf8(skill_md).unwrap(),
        ),
        ("@main/skills/pdf/scripts/run.py", "py!".to_string()),
    ];
    let base = spawn_mock(routes);
    let client = MarketClient::with_bases(&base, &format!("{base}/raw-404"), &base, &base);
    let files = client
        .fetch_skill_files("mocksrc/repo", "main", "skills/pdf")
        .await
        .unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].rel_path, "SKILL.md");
    assert_eq!(files[1].rel_path, "scripts/run.py");
    assert_eq!(files[0].bytes, b"# PDF \xe6\x8a\x80\xe8\x83\xbd".to_vec());

    // 缺 SKILL.md 拒绝。
    let listing = serde_json::json!({
        "files": [{ "name": "/skills/pdf/README.md", "size": 3, "type": "file" }]
    })
    .to_string();
    let base = spawn_mock(vec![
        ("/v1/packages/gh/", listing),
        ("@main/skills/pdf/README.md", "abc".to_string()),
    ]);
    let client = MarketClient::with_bases(&base, &base, &base, &base);
    let err = client
        .fetch_skill_files("mocksrc/repo", "main", "skills/pdf")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("SKILL.md"));

    // 超上限文件数拒绝（65 个文件 > 64）。
    let files_json: Vec<serde_json::Value> = (0..65)
        .map(|i| {
            serde_json::json!({
                "name": format!("/skills/pdf/f{i}.txt"), "size": 1, "type": "file"
            })
        })
        .collect();
    let base = spawn_mock(vec![(
        "/v1/packages/gh/",
        serde_json::json!({ "files": files_json }).to_string(),
    )]);
    let client = MarketClient::with_bases(&base, &base, &base, &base);
    let err = client
        .fetch_skill_files("mocksrc/repo", "main", "skills/pdf")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("上限"));
}

#[tokio::test]
async fn github_trees_fallback_serves_listing() {
    let trees = serde_json::json!({
        "tree": [
            { "path": "skills/pdf/SKILL.md", "type": "blob", "size": 3 }
        ]
    })
    .to_string();
    let base = spawn_mock(vec![
        ("/git/trees", trees),
        ("/main/skills/pdf/SKILL.md", "abc".to_string()),
    ]);
    // data API 失败（未注册路由 404）→ GitHub API 回退；内容走 raw（无 @）。
    let client = MarketClient::with_bases(&format!("{base}/cdn-404"), &base, &base, &base);
    let files = client
        .fetch_skill_files("mocksrc/repo", "main", "skills/pdf")
        .await
        .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].rel_path, "SKILL.md");
}

#[test]
fn install_skill_dir_writes_sidecar_and_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("skills");
    let files = vec![
        market::SkillFile {
            rel_path: "SKILL.md".into(),
            bytes: b"---\nname: pdf\n---\nbody".to_vec(),
        },
        market::SkillFile {
            rel_path: "scripts/run.py".into(),
            bytes: b"py!".to_vec(),
        },
    ];
    let sidecar = MarketSidecar {
        market_source: "owner/market".into(),
        source: "owner/repo".into(),
        path: "skills/pdf".into(),
        git_ref: "main".into(),
        version: Some("1.2.0".into()),
        installed_at: "2026-10-06T00:00:00Z".into(),
    };
    let dir = install_skill_dir(&root, "pdf", &files, &sidecar).unwrap();
    assert_eq!(dir, root.join("pdf"));
    assert!(dir.join("SKILL.md").is_file());
    assert!(dir.join("scripts/run.py").is_file());
    let got = read_sidecar(&dir).unwrap();
    assert_eq!(got.version.as_deref(), Some("1.2.0"));
    assert_eq!(got.source, "owner/repo");

    // 更新：同 source+path 覆盖重写，旧文件清除、无 staging 残留。
    let files_v2 = vec![market::SkillFile {
        rel_path: "SKILL.md".into(),
        bytes: b"v2".to_vec(),
    }];
    let sidecar_v2 = MarketSidecar {
        version: Some("2.0.0".into()),
        ..sidecar.clone()
    };
    let dir = install_skill_dir(&root, "pdf", &files_v2, &sidecar_v2).unwrap();
    assert!(!dir.join("scripts").exists(), "更新后旧文件不残留");
    assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), "v2");
    assert_eq!(
        read_sidecar(&dir).unwrap().version.as_deref(),
        Some("2.0.0")
    );
    assert!(!root.join(".tenon-install-pdf").exists(), "staging 不残留");

    // 非市场技能目录 read_sidecar 返回 None。
    let manual = root.join("manual");
    std::fs::create_dir_all(&manual).unwrap();
    std::fs::write(manual.join("SKILL.md"), "x").unwrap();
    assert!(read_sidecar(&manual).is_none());

    // 非法技能名拒绝。
    assert!(install_skill_dir(&root, "../escape", &files, &sidecar).is_err());
}

#[test]
fn source_and_name_validators() {
    assert!(market::is_valid_source("owner/repo"));
    assert!(market::is_valid_source("a.b_c/d-e.f"));
    assert!(!market::is_valid_source("owner"));
    assert!(!market::is_valid_source("owner/../etc"));
    assert!(!market::is_valid_source("owner/repo/x"));

    assert!(market::is_valid_mcp_name("github"));
    assert!(market::is_valid_mcp_name("my-server2"));
    assert!(!market::is_valid_mcp_name("my_server"));
    assert!(!market::is_valid_mcp_name("Github"));
    assert!(!market::is_valid_mcp_name(""));
    assert!(!market::is_valid_mcp_name("-lead"));
}
