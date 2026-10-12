//! tenon-acp-agent（v2.26，design-v2.md §8②）：ACP agent 二进制——被 Zed /
//! JetBrains 等编辑器按 ACP agent 配置拉起，经 HTTP 连 Tenon daemon，把编辑器
//! prompt 转为 Tenon 会话任务、把任务产出流式回发编辑器。编辑器由此获得完整
//! Tenon harness（档位 / 快照 / 上下文引擎全在会话链内）。
//!
//! 配置：env `TENON_URL`（如 `http://127.0.0.1:9876`）与 `TENON_TOKEN`。

use std::io::Write;
use tenon_acp::server::{AcpBackend, AcpServer};

struct DaemonBackend {
    base: String,
    token: String,
    client: reqwest::blocking::Client,
    session_id: Option<String>,
    /// trace 增量游标。
    after: i64,
}

impl DaemonBackend {
    fn from_env() -> Result<Self, String> {
        let base =
            std::env::var("TENON_URL").map_err(|_| "缺 TENON_URL（daemon 基址）".to_string())?;
        let token = std::env::var("TENON_TOKEN").map_err(|_| "缺 TENON_TOKEN".to_string())?;
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(h) = reqwest::header::HeaderValue::from_str(&token) {
            headers.insert("X-Tenon-Token", h);
        }
        let client = reqwest::blocking::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.clone(),
            client,
            session_id: None,
            after: 0,
        })
    }
}

impl AcpBackend for DaemonBackend {
    fn capabilities(&self) -> serde_json::Value {
        // Tenon harness 能力：会话可恢复由 daemon 承载，ACP 面静态声明
        serde_json::json!({"loadSession": false})
    }

    fn new_session(&mut self, cwd: &str) -> Result<String, String> {
        // /projects/open 登记 cwd → POST /session 建会话
        let project: serde_json::Value = self
            .client
            .post(format!("{}/projects/open", self.base))
            .header("X-Tenon-Token", &self.token)
            .json(&serde_json::json!({"path": cwd}))
            .send()
            .map_err(|e| e.to_string())?
            .json()
            .map_err(|e| e.to_string())?;
        let pid = project
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("projects/open 未返回 id")?;
        let session: serde_json::Value = self
            .client
            .post(format!("{}/session", self.base))
            .header("X-Tenon-Token", &self.token)
            .json(&serde_json::json!({"project_id": pid}))
            .send()
            .map_err(|e| e.to_string())?
            .json()
            .map_err(|e| e.to_string())?;
        let sid = session
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or("session 未返回 session_id")?
            .to_string();
        self.session_id = Some(sid.clone());
        self.after = 0;
        Ok(sid)
    }

    fn prompt(
        &mut self,
        session_id: &str,
        text: &str,
        chunk: tenon_acp::server::ChunkSink,
    ) -> Result<String, String> {
        // 发消息（运行态入队语义由 daemon 承载）
        let mut req = self
            .client
            .post(format!("{}/session/{session_id}/message", self.base));
        req = req.header("X-Tenon-Token", &self.token);
        req.json(&serde_json::json!({"text": text}))
            .send()
            .map_err(|e| e.to_string())?;
        // 轮询 /trace 增量：decision（intent 文本）按行拆 chunk；终态映射 stopReason
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            if std::time::Instant::now() > deadline {
                return Err("任务 600s 未终态".into());
            }
            let mut req = self.client.get(format!(
                "{}/session/{session_id}/trace?after={}",
                self.base, self.after
            ));
            req = req.header("X-Tenon-Token", &self.token);
            let trace: serde_json::Value = req
                .send()
                .map_err(|e| e.to_string())?
                .json()
                .map_err(|e| e.to_string())?;
            let events = trace
                .get("events")
                .and_then(|e| e.as_array())
                .cloned()
                .unwrap_or_default();
            for ev in &events {
                if let Some(seq) = ev.get("seq").and_then(|s| s.as_i64()) {
                    self.after = self.after.max(seq);
                }
                if ev.get("type").and_then(|t| t.as_str()) == Some("decision") {
                    if let Some(intent) = ev.pointer("/payload/intent").and_then(|i| i.as_str()) {
                        if !intent.trim().is_empty() {
                            chunk(intent);
                        }
                    }
                }
            }
            // 会话终态
            let mut req = self
                .client
                .get(format!("{}/session/{session_id}", self.base));
            req = req.header("X-Tenon-Token", &self.token);
            let st: serde_json::Value = req
                .send()
                .map_err(|e| e.to_string())?
                .json()
                .map_err(|e| e.to_string())?;
            match st.get("status").and_then(|s| s.as_str()) {
                Some("done") => return Ok("end_turn".into()),
                Some("error") => return Ok("refusal".into()),
                Some("paused") => return Ok("paused".into()),
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
}

fn main() {
    let mut backend = match DaemonBackend::from_env() {
        Ok(b) => b,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "tenon-acp-agent: {e}");
            std::process::exit(2);
        }
    };
    let stdin = std::io::BufReader::new(std::io::stdin().lock());
    let stdout = std::io::stdout().lock();
    if let Err(e) = AcpServer::run(stdin, stdout, &mut backend) {
        let _ = writeln!(std::io::stderr(), "tenon-acp-agent: {e}");
        std::process::exit(1);
    }
}
