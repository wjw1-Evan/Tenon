//! 语言包语义端点集成测试（§8.5 / §8.3：共享 LSP 宿主）。
//! 真实语言服务器（typescript-language-server / pyright）；未安装时跳过。

use std::path::Path;
use std::time::Duration;

use tenon_lsp::manager::command_on_path;
use tenon_lsp::manager::{LspManager, LspManagerError};

fn skip_if_missing(binary: &str) -> bool {
    if !command_on_path(binary) {
        eprintln!("跳过：{binary} 未安装");
        return true;
    }
    false
}

async fn semantic(
    mgr: &LspManager,
    root: &Path,
    file: &str,
    op: &str,
    line: u32,
    character: u32,
) -> Result<serde_json::Value, LspManagerError> {
    // 首次请求包含语言服务器启动与项目加载（REQUEST_TIMEOUT 45s 足够）
    mgr.request(root, file, op, line, character).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typescript_pack_hover_definition_references_completion() {
    if skip_if_missing("typescript-language-server") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"name":"t","version":"0.1.0"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/math.ts"),
        "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/app.ts"),
        "import { add } from './math';\nexport const sum = add(1, 2);\n",
    )
    .unwrap();

    let mgr = LspManager::new();
    let root = dir.path().to_path_buf();

    // hover：add 声明处（src/math.ts 第 0 行，`add` 起始 ~ char 16）
    let hover = semantic(&mgr, &root, "src/math.ts", "hover", 0, 18)
        .await
        .expect("hover 请求");
    assert!(
        hover.get("contents").is_some(),
        "hover 应返回 contents: {hover}"
    );

    // definition：app.ts 中 `add(1, 2)` 的调用点（行 1，char 19 = `add` 起始）→ 定义在 math.ts
    let def = semantic(&mgr, &root, "src/app.ts", "definition", 1, 19)
        .await
        .expect("definition 请求");
    let def_json: Vec<serde_json::Value> = if def.is_array() {
        serde_json::from_value(def).unwrap_or_default()
    } else {
        vec![def]
    };
    assert!(
        !def_json.is_empty() && def_json[0]["uri"].as_str().unwrap_or("").ends_with(".ts"),
        "定义应返回至少一个 TS 位置: {def_json:?}"
    );

    // references：add 声明处的引用（math.ts 声明 + app.ts 调用）。
    // tsserver 对新打开文件的引用索引有延迟：轮询等待至多 15s
    let mut arr: Vec<serde_json::Value> = Vec::new();
    for _ in 0..15 {
        let refs = semantic(&mgr, &root, "src/math.ts", "references", 0, 18)
            .await
            .expect("references 请求");
        arr = refs.as_array().cloned().unwrap_or_default();
        if arr.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(arr.len() >= 2, "add 应有 ≥2 处引用（声明+调用）: {arr:?}");

    // completion：math.ts 内输入位置
    let completion = semantic(&mgr, &root, "src/math.ts", "completion", 1, 2)
        .await
        .expect("completion 请求");
    let items = if completion.is_array() {
        completion
    } else {
        completion.get("items").cloned().unwrap_or_default()
    };
    assert!(
        items.as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "补全列表非空"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typescript_pack_diagnostics_report_type_errors() {
    if skip_if_missing("typescript-language-server") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"name":"t2","version":"0.1.0"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("bad.ts"),
        "const x: number = \"definitely not a number\";\n",
    )
    .unwrap();

    let mgr = LspManager::new();
    let diag = mgr
        .request(dir.path(), "bad.ts", "diagnostics", 0, 0)
        .await
        .expect("diagnostics 请求");
    let items = diag
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!items.is_empty(), "类型错误应产生诊断: {diag}");
    assert!(
        items
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("number")),
        "诊断应包含 number 类型错误: {items:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn python_pack_hover_and_diagnostics() {
    if skip_if_missing("pyright-langserver") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = 't'\n").unwrap();
    std::fs::write(
        dir.path().join("main.py"),
        "def greet(name: str) -> str:\n    return 'hello ' + name\n\n\ngreet(123)\n",
    )
    .unwrap();

    let mgr = LspManager::new();
    // hover：greet 函数名（行 0，char 5）
    let hover = mgr
        .request(dir.path(), "main.py", "hover", 0, 5)
        .await
        .expect("hover 请求");
    assert!(hover.get("contents").is_some(), "pyright hover: {hover}");

    // diagnostics：greet(123) 参数类型错误
    let diag = mgr
        .request(dir.path(), "main.py", "diagnostics", 0, 0)
        .await
        .expect("diagnostics 请求");
    let items = diag
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        items.iter().any(|d| {
            let m = d["message"].as_str().unwrap_or("");
            m.contains("int") || m.contains("123")
        }),
        "pyright 应报告实参类型错误: {items:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_server_returns_unavailable() {
    // 命令不存在的语言包 → 明确 503 语义（安装向导引导）
    let dir = tempfile::tempdir().unwrap();
    // rust-analyzer 通常未随 cargo 安装；若恰好存在则跳过本用例
    if command_on_path("rust-analyzer") {
        eprintln!("跳过：rust-analyzer 存在");
        return;
    }
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname='x'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    let mgr = LspManager::new();
    let err = mgr
        .request(dir.path(), "src/lib.rs", "hover", 0, 0)
        .await
        .expect_err("应报语言包不可用");
    assert!(err.to_string().contains("未安装"), "{err}");
}
