//! 浏览器宿主集成测试（§7.6 v2.79）：真 Chrome / Chromium 无头实例 + 本地假页面。
//!
//! 未安装 Chrome / Chromium 的机器（含部分 CI）跳过——探测不到即 `eprintln!`
//! 说明后返回，不假装通过（同 GLM 真实端点联调测试的既有口径）。
//! 覆盖：打开并等加载完成、页面 console / 未捕获异常捕获、clear 语义、
//! JS 求值（基本值 / DOM 读取）、未知 target 拒绝、关闭页面、未打开即求值。

use std::time::Duration;

use axum::response::Response;
use axum::routing::get;
use tenon_agent::browser::BrowserHost;
use tenon_daemon::browser::{detect_chrome, ChromeBrowser};

/// 探测不到浏览器 → None（测试跳过）。
fn host_or_skip() -> Option<ChromeBrowser> {
    match detect_chrome() {
        Some(p) => {
            eprintln!("使用浏览器: {}", p.display());
            Some(ChromeBrowser::new(None))
        }
        None => {
            eprintln!("跳过：未探测到 Chrome / Chromium（TENON_BROWSER_PATH 可显式指定）");
            None
        }
    }
}

/// 假页面：标题 + 一条 console.error + 一条延迟 console.log + 一个 DOM 探针。
async fn start_page_server() -> u16 {
    async fn index() -> Response {
        axum::response::IntoResponse::into_response((
            [("content-type", "text/html; charset=utf-8")],
            r#"<html><head><title>Tenon 预览测试</title></head><body>
<div id="app">hello</div>
<script>
console.error("boom-from-page");
setTimeout(function(){ console.log("later-log"); }, 60);
window.__tenonProbe = function(){ return document.querySelectorAll("div").length; };
setTimeout(function(){ window.__tenonReady = true; }, 80);
</script></body></html>"#,
        ))
    }
    let app = axum::Router::new().route("/", get(index));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    port
}

#[tokio::test]
async fn browser_open_console_eval_close_full_chain() {
    let Some(host) = host_or_skip() else { return };
    let port = start_page_server().await;

    let page = host
        .open(&format!("http://127.0.0.1:{port}/"))
        .await
        .expect("打开页面");
    assert_eq!(page.title, "Tenon 预览测试", "标题应取回");
    assert!(
        page.url.starts_with(&format!("http://127.0.0.1:{port}")),
        "URL 应取回: {}",
        page.url
    );
    assert!(!page.target_id.is_empty());

    // 等延迟 log 进入缓冲（页面 setTimeout 60ms）
    tokio::time::sleep(Duration::from_millis(500)).await;
    let logs = host.console(None, false).await.expect("读控制台");
    let text = logs
        .iter()
        .map(|e| format!("[{}] {}", e.level, e.text))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("boom-from-page"),
        "页面 console.error 须捕获: {text}"
    );
    assert!(
        text.contains("later-log"),
        "延迟 console.log 须捕获: {text}"
    );
    assert!(
        logs.iter().any(|e| e.level == "error"),
        "级别须保留: {text}"
    );

    // clear 语义：本次返回已读内容，之后缓冲为空
    let cleared = host.console(None, true).await.unwrap();
    assert!(!cleared.is_empty());
    assert!(host.console(None, false).await.unwrap().is_empty());

    // JS 求值：基本值 / DOM / 运行时状态
    assert_eq!(host.eval(None, "1+1").await.unwrap(), "2");
    assert_eq!(
        host.eval(None, "document.querySelector('#app').textContent")
            .await
            .unwrap(),
        "hello"
    );
    assert_eq!(host.eval(None, "window.__tenonProbe()").await.unwrap(), "1");
    assert_eq!(
        host.eval(None, "document.title").await.unwrap(),
        "Tenon 预览测试"
    );

    // 求值抛异常 → 软错误（连接仍可用）
    let err = host
        .eval(None, "throw new Error('nope')")
        .await
        .unwrap_err();
    assert!(err.contains("nope"), "异常须如实回报: {err}");
    assert_eq!(
        host.eval(None, "2+2").await.unwrap(),
        "4",
        "异常后连接仍可用"
    );

    // 未知 target → 拒绝且列出当前页面
    let err = host.eval(Some("no-such-target"), "1").await.unwrap_err();
    assert!(err.contains("未知 target_id"), "{err}");

    // 关闭
    let closed = host.close(None).await.unwrap();
    assert_eq!(closed, vec![page.target_id.clone()]);
    // 关掉后再求值 → 提示先打开页面
    let err = host.eval(None, "1").await.unwrap_err();
    assert!(err.contains("browser_open"), "{err}");
}

#[tokio::test]
async fn browser_eval_before_open_is_honest_error() {
    let Some(host) = host_or_skip() else { return };
    // 未打开任何页面时求值 / 读控制台 / 关闭都如实报错，不静默启动浏览器
    let err = host.eval(None, "1").await.unwrap_err();
    assert!(err.contains("browser_open"), "{err}");
    let err = host.console(None, false).await.unwrap_err();
    assert!(err.contains("browser_open"), "{err}");
    assert!(host.close(None).await.unwrap().is_empty());
}
