//! 浏览器宿主抽象（§7.6 v2.80）：Agent 侧 `browser_*` 工具的宿主接口。
//!
//! 内核只依赖本 trait（同 `lsp` / `mcp` 的注入形态），实现在 daemon——
//! `tenon-daemon::browser` 以 CDP 驱动本机 Chrome / Chromium 的无头实例。
//! 这样做的理由：驱动浏览器是重 IO 的进程级设施，内核（tenon-agent）不该
//! 绑死具体协议；测试可注入假实现验证工具面接线。
//!
//! 分级（§9.2）：`browser_open` / `browser_eval` 为 C 级（驱动浏览器即出网面，
//! 与 `http_fetch` 同轨、只读会话禁用）；`browser_console` / `browser_close`
//! 为 A 级（只读本地缓冲 / 只回收自己开的页面）。

use async_trait::async_trait;
use serde::Serialize;

/// 已打开的浏览器页面。
#[derive(Debug, Clone, Serialize)]
pub struct BrowserPage {
    /// 页面句柄（CDP target id）；后续工具调用经它指定页面。
    pub target_id: String,
    pub url: String,
    pub title: String,
}

/// 页面控制台条目（console 五级 / 未捕获异常 / 未处理 Promise 拒绝）。
#[derive(Debug, Clone, Serialize)]
pub struct ConsoleEntry {
    /// log | info | warn | error | debug
    pub level: String,
    pub text: String,
    /// 毫秒时间戳（Unix epoch）
    pub ts_ms: i64,
}

/// 浏览器宿主。实现须自行处理"首次调用时才启动浏览器"的惰性语义。
#[async_trait]
pub trait BrowserHost: Send + Sync + 'static {
    /// 打开页面并等待加载完成；`url` 须为 http(s)。
    async fn open(&self, url: &str) -> Result<BrowserPage, String>;
    /// 在页面求值 JS（`returnByValue`），返回值的 JSON 文本。
    async fn eval(&self, target: Option<&str>, expression: &str) -> Result<String, String>;
    /// 读页面控制台缓冲；`clear` = 读后清空。
    async fn console(&self, target: Option<&str>, clear: bool)
        -> Result<Vec<ConsoleEntry>, String>;
    /// 关闭页面（`target` 省略 = 关闭全部页面），返回被关闭的 target id。
    async fn close(&self, target: Option<&str>) -> Result<Vec<String>, String>;
}

#[cfg(test)]
pub(crate) mod fake {
    //! 假宿主（内核测试注入）：记录调用、返回固定结果，供 executor 工具面测试。
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeBrowser {
        pub opened: Mutex<Vec<String>>,
        pub closed: Mutex<Vec<String>>,
        pub logs: Mutex<Vec<ConsoleEntry>>,
    }

    #[async_trait]
    impl BrowserHost for FakeBrowser {
        async fn open(&self, url: &str) -> Result<BrowserPage, String> {
            self.opened.lock().unwrap().push(url.to_string());
            Ok(BrowserPage {
                target_id: "t1".into(),
                url: url.to_string(),
                title: "Fake".into(),
            })
        }
        async fn eval(&self, _t: Option<&str>, expression: &str) -> Result<String, String> {
            Ok(format!("eval:{expression}"))
        }
        async fn console(
            &self,
            _t: Option<&str>,
            clear: bool,
        ) -> Result<Vec<ConsoleEntry>, String> {
            let mut logs = self.logs.lock().unwrap();
            let out = logs.clone();
            if clear {
                logs.clear();
            }
            Ok(out)
        }
        async fn close(&self, target: Option<&str>) -> Result<Vec<String>, String> {
            self.closed
                .lock()
                .unwrap()
                .push(target.unwrap_or("all").to_string());
            Ok(vec![target.unwrap_or("all").to_string()])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeBrowser;
    use super::*;

    #[tokio::test]
    async fn fake_host_satisfies_trait_contract() {
        let host = FakeBrowser::default();
        host.logs.lock().unwrap().push(ConsoleEntry {
            level: "error".into(),
            text: "boom".into(),
            ts_ms: 1,
        });
        let page = host.open("http://127.0.0.1:3000/").await.unwrap();
        assert_eq!(page.target_id, "t1");
        assert_eq!(
            host.opened.lock().unwrap().as_slice(),
            ["http://127.0.0.1:3000/"]
        );
        assert_eq!(host.eval(None, "1+1").await.unwrap(), "eval:1+1");
        assert_eq!(host.console(None, false).await.unwrap().len(), 1);
        assert!(host.console(None, true).await.unwrap().len() == 1);
        assert!(host.console(None, false).await.unwrap().is_empty());
        assert_eq!(host.close(None).await.unwrap(), vec!["all".to_string()]);
    }
}
