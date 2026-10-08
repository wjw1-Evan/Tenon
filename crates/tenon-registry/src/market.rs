//! GitHub 技能与插件市场客户端（设计方案 §13.5，v1.145）。
//!
//! 市场源 = GitHub 仓库 `owner/repo`，根目录（或 `.tenon/`）含 `marketplace.json`
//! 清单，列出 skill / mcp 两类条目。拉取走多镜像链（任一可用即止，国内可达性
//! 优先，v1.102 Laya 同法）：
//! - 清单：jsDelivr CDN → raw.githubusercontent；
//! - 技能目录列举：jsDelivr data API → GitHub API git/trees；
//! - 技能文件内容：jsDelivr CDN → raw.githubusercontent。
//!
//! 信任模型（§12.5 社区通道）：无签名，安全靠安装期硬校验——mcp 启动器白名单、
//! argv 无控制字符（无 shell 执行面）、env 仅 `env:VAR` 引用、路径规范形校验。

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_GIT_REF: &str = "main";

/// mcp 条目启动器白名单（§13.5：argv 直启，无 shell 面）。
pub const LAUNCHER_WHITELIST: &[&str] =
    &["npx", "uvx", "bunx", "node", "python", "python3", "docker"];

/// 安装上限（§13.5）。
pub const MAX_FILES_PER_SKILL: usize = 64;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    #[error("清单/文件不可达（镜像链全部失败）: {0}")]
    Unreachable(String),
    #[error("清单格式非法: {0}")]
    BadManifest(String),
    #[error("条目校验失败: {0}")]
    BadEntry(String),
    #[error("非法路径或引用: {0}")]
    BadPath(String),
    #[error("技能目录超出上限（≤{MAX_FILES_PER_SKILL} 文件 / 单文件 ≤1MB / 总量 ≤4MB）")]
    TooLarge,
    #[error("技能目录缺少 SKILL.md")]
    MissingSkillMd,
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 市场源形态：`owner/repo`（不含 `..`）。
pub fn is_valid_source(source: &str) -> bool {
    let Some((owner, repo)) = source.split_once('/') else {
        return false;
    };
    valid_seg(owner) && valid_seg(repo)
}

/// §13.4 技能规范 id。
pub fn is_valid_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !name.is_empty()
        && name.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// mcp 服务器名：`^[a-z][a-z0-9-]{0,31}$`（禁下划线，保证工具命名解析无歧义）。
pub fn is_valid_mcp_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=32).contains(&name.len())
        && bytes[0].is_ascii_lowercase()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_seg(seg: &str) -> bool {
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 仓库内相对路径（拒绝绝对路径、`..` 与反斜杠）。
fn is_valid_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(valid_seg)
}

/// git ref（分支 / tag / commit；拒绝 `..`、空白、控制字符与 URL 特殊字符）。
fn is_valid_git_ref(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 256
        && !r.contains("..")
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'))
        && !r.starts_with('/')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketKind {
    Skill,
    Mcp,
}

/// 市场条目（marketplace.json `entries` 元素）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketEntry {
    pub kind: MarketKind,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 内容来源仓库；缺省 = 市场源仓库自身。
    #[serde(default)]
    pub source: Option<String>,
    /// 仓库内技能目录（skill 必填）。
    #[serde(default)]
    pub path: Option<String>,
    /// 分支 / tag / commit；缺省 main。
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    // ---- mcp 专用 ----
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 当前仅 `net:*`。
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// 市场清单（marketplace.json）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketManifest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entries: Vec<MarketEntry>,
}

impl MarketEntry {
    /// 内容来源仓库（source 缺省回退市场源）。
    pub fn resolved_source(&self, market_source: &str) -> String {
        self.source
            .clone()
            .unwrap_or_else(|| market_source.to_string())
    }

    pub fn resolved_ref(&self) -> &str {
        self.git_ref.as_deref().unwrap_or(DEFAULT_GIT_REF)
    }

    /// 安装期校验（§13.5 硬约束；违者拒绝安装）。
    pub fn validate(&self, market_source: &str) -> Result<(), MarketError> {
        if !is_valid_source(market_source) {
            return Err(MarketError::BadEntry(format!(
                "市场源形态非法: {market_source}"
            )));
        }
        if !is_valid_source(&self.resolved_source(market_source)) {
            return Err(MarketError::BadEntry(format!(
                "条目来源形态非法: {}",
                self.resolved_source(market_source)
            )));
        }
        if let Some(r) = &self.git_ref {
            if !is_valid_git_ref(r) {
                return Err(MarketError::BadPath(format!("ref 非法: {r}")));
            }
        }
        match self.kind {
            MarketKind::Skill => {
                if !is_valid_skill_name(&self.name) {
                    return Err(MarketError::BadEntry(format!("技能名非法: {}", self.name)));
                }
                let path = self
                    .path
                    .as_deref()
                    .ok_or_else(|| MarketError::BadEntry("skill 条目缺少 path".to_string()))?;
                if !is_valid_repo_path(path) {
                    return Err(MarketError::BadPath(format!("path 非法: {path}")));
                }
            }
            MarketKind::Mcp => {
                if !is_valid_mcp_name(&self.name) {
                    return Err(MarketError::BadEntry(format!(
                        "mcp 服务器名非法（^[a-z][a-z0-9-]{{0,31}}$）: {}",
                        self.name
                    )));
                }
                let command = self
                    .command
                    .as_deref()
                    .ok_or_else(|| MarketError::BadEntry("mcp 条目缺少 command".to_string()))?;
                if !LAUNCHER_WHITELIST.contains(&command) {
                    return Err(MarketError::BadEntry(format!(
                        "启动器不在白名单 {LAUNCHER_WHITELIST:?}: {command}"
                    )));
                }
                for arg in &self.args {
                    // argv 直启无 shell 解释面：仅拒控制字符；
                    // `/` 等分隔符合法（npm 包名 @scope/pkg、路径参数均含）。
                    if arg.chars().any(|c| c.is_control()) {
                        return Err(MarketError::BadEntry(format!("args 含控制字符: {arg}")));
                    }
                }
                for (k, v) in &self.env {
                    let valid_key = !k.is_empty()
                        && k.chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                    let Some(var) = v.strip_prefix("env:") else {
                        return Err(MarketError::BadEntry(format!(
                            "env 值仅允许 env:VAR 引用: {k}"
                        )));
                    };
                    if !valid_key
                        || var.is_empty()
                        || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        return Err(MarketError::BadEntry(format!("env 键值非法: {k}={v}")));
                    }
                }
                for p in &self.permissions {
                    if p != "net:*" {
                        return Err(MarketError::BadEntry(format!(
                            "permissions 当前仅支持 net:*: {p}"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// 技能安装 sidecar（`~/.tenon/skills/<name>/.tenon-market.json`，§13.5 溯源）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSidecar {
    pub market_source: String,
    pub source: String,
    pub path: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub version: Option<String>,
    pub installed_at: String,
}

/// 拉取的技能文件（rel = 技能目录内相对路径，POSIX 分隔）。
#[derive(Debug, Clone)]
pub struct SkillFile {
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// 市场客户端（镜像链 base 可注入供测试指向本地 mock）。
pub struct MarketClient {
    http: reqwest::Client,
    /// jsDelivr CDN（文件内容）：{base}/{source}@{ref}/{path}
    cdn_base: String,
    /// raw.githubusercontent（清单与文件回退）：{base}/{source}/{ref}/{path}
    raw_base: String,
    /// jsDelivr data API（目录列举）：{base}/v1/packages/gh/{source}@{ref}
    data_base: String,
    /// GitHub API（目录列举回退）：{base}/repos/{source}/git/trees/{ref}
    api_base: String,
}

impl Default for MarketClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            cdn_base: "https://cdn.jsdelivr.net/gh".into(),
            raw_base: "https://raw.githubusercontent.com".into(),
            data_base: "https://data.jsdelivr.com".into(),
            api_base: "https://api.github.com".into(),
        }
    }

    /// 测试注入本地 mock base。
    pub fn with_bases(cdn: &str, raw: &str, data: &str, api: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            cdn_base: cdn.into(),
            raw_base: raw.into(),
            data_base: data.into(),
            api_base: api.into(),
        }
    }

    async fn get_text(&self, urls: &[String]) -> Result<String, MarketError> {
        let mut last = String::new();
        for url in urls {
            match self.http.get(url).send().await {
                Ok(resp) if resp.status().is_success() => match resp.text().await {
                    Ok(text) => return Ok(text),
                    Err(e) => last = format!("{url}: {e}"),
                },
                Ok(resp) => last = format!("{url}: HTTP {}", resp.status()),
                Err(e) => last = format!("{url}: {e}"),
            }
        }
        Err(MarketError::Unreachable(last))
    }

    /// 拉取市场清单（先根目录后 `.tenon/`，每处 jsDelivr → raw）。
    pub async fn fetch_manifest(
        &self,
        market_source: &str,
        git_ref: &str,
    ) -> Result<MarketManifest, MarketError> {
        if !is_valid_source(market_source) || !is_valid_git_ref(git_ref) {
            return Err(MarketError::BadPath(format!("{market_source}@{git_ref}")));
        }
        let mut urls = Vec::new();
        for file in ["marketplace.json", ".tenon/marketplace.json"] {
            urls.push(format!(
                "{}/{market_source}@{git_ref}/{file}",
                self.cdn_base
            ));
            urls.push(format!(
                "{}/{market_source}/{git_ref}/{file}",
                self.raw_base
            ));
        }
        let text = self.get_text(&urls).await?;
        serde_json::from_str(&text).map_err(|e| MarketError::BadManifest(e.to_string()))
    }

    /// 列举技能目录文件（镜像链：jsDelivr data API → GitHub API trees）。
    /// 返回 (仓库内完整路径, 字节数) 列表。
    async fn list_skill_files(
        &self,
        source: &str,
        git_ref: &str,
        path: &str,
    ) -> Result<Vec<(String, usize)>, MarketError> {
        let prefix = format!("/{path}/");
        // jsDelivr data API：files[].name 以 "/" 开头。
        let data_url = format!(
            "{}/v1/packages/gh/{source}@{git_ref}?structure=flat",
            self.data_base
        );
        if let Ok(text) = self.get_text(&[data_url]).await {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                let files = value.get("files").and_then(|f| f.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|f| {
                            let name = f.get("name")?.as_str()?;
                            let size = f.get("size")?.as_u64()? as usize;
                            let ty = f.get("type").and_then(|t| t.as_str()).unwrap_or("file");
                            (ty == "file" && name.starts_with(&prefix))
                                .then(|| (name.trim_start_matches('/').to_string(), size))
                        })
                        .collect::<Vec<_>>()
                });
                if let Some(files) = files {
                    if !files.is_empty() {
                        return Ok(files);
                    }
                }
            }
        }
        // GitHub API trees 回退。
        let api_url = format!(
            "{}/repos/{source}/git/trees/{git_ref}?recursive=1",
            self.api_base
        );
        let text = self.get_text(&[api_url]).await?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| MarketError::BadManifest(e.to_string()))?;
        // truncated=true 表示递归列举被截断（仓库过大）：静默接受会装出
        // 缺文件的残缺技能，必须显式报错走下一镜像
        if value.get("truncated").and_then(|t| t.as_bool()) == Some(true) {
            return Err(MarketError::BadManifest(
                "git trees 列举被截断（truncated=true），目录不完整".into(),
            ));
        }
        let files = value
            .get("tree")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| {
                        let p = f.get("path")?.as_str()?;
                        let ty = f.get("type").and_then(|t| t.as_str()).unwrap_or("blob");
                        let size = f.get("size").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
                        (ty == "blob" && p.starts_with(&format!("{path}/")))
                            .then(|| (p.to_string(), size))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if files.is_empty() {
            return Err(MarketError::MissingSkillMd);
        }
        Ok(files)
    }

    /// 拉取技能目录全部文件（上限见 [`MAX_FILES_PER_SKILL`] 等）。
    pub async fn fetch_skill_files(
        &self,
        source: &str,
        git_ref: &str,
        path: &str,
    ) -> Result<Vec<SkillFile>, MarketError> {
        if !is_valid_source(source) || !is_valid_git_ref(git_ref) || !is_valid_repo_path(path) {
            return Err(MarketError::BadPath(format!("{source}@{git_ref}:{path}")));
        }
        let files = self.list_skill_files(source, git_ref, path).await?;
        if files.len() > MAX_FILES_PER_SKILL {
            return Err(MarketError::TooLarge);
        }
        let prefix_len = path.len() + 1;
        let mut out = Vec::with_capacity(files.len());
        let mut total = 0usize;
        for (full, size) in files {
            if size > MAX_FILE_BYTES {
                return Err(MarketError::TooLarge);
            }
            total += size;
            if total > MAX_TOTAL_BYTES {
                return Err(MarketError::TooLarge);
            }
            let rel = full[prefix_len..].to_string();
            let urls = vec![
                format!("{}/{source}@{git_ref}/{full}", self.cdn_base),
                format!("{}/{source}/{git_ref}/{full}", self.raw_base),
            ];
            let bytes = self.get_bytes(&urls).await?;
            out.push(SkillFile {
                rel_path: rel,
                bytes,
            });
        }
        if !out.iter().any(|f| f.rel_path == "SKILL.md") {
            return Err(MarketError::MissingSkillMd);
        }
        Ok(out)
    }

    async fn get_bytes(&self, urls: &[String]) -> Result<Vec<u8>, MarketError> {
        let mut last = String::new();
        for url in urls {
            match self.http.get(url).send().await {
                Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                    Ok(bytes) => return Ok(bytes.to_vec()),
                    Err(e) => last = format!("{url}: {e}"),
                },
                Ok(resp) => last = format!("{url}: HTTP {}", resp.status()),
                Err(e) => last = format!("{url}: {e}"),
            }
        }
        Err(MarketError::Unreachable(last))
    }
}

/// 落盘技能目录：写入 files + sidecar（更新语义 = 先清空目标目录再写入；
/// 经 skills_root 内临时目录 + rename 保证半成品不外见）。
/// 返回技能目录路径。
pub fn install_skill_dir(
    skills_root: &Path,
    name: &str,
    files: &[SkillFile],
    sidecar: &MarketSidecar,
) -> Result<std::path::PathBuf, MarketError> {
    if !is_valid_skill_name(name) {
        return Err(MarketError::BadEntry(format!("技能名非法: {name}")));
    }
    std::fs::create_dir_all(skills_root)?;
    let target = skills_root.join(name);
    let staging = skills_root.join(format!(".tenon-install-{name}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    for file in files {
        let dest = staging.join(&file.rel_path);
        if !dest.starts_with(&staging) {
            return Err(MarketError::BadPath(file.rel_path.clone()));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &file.bytes)?;
    }
    let sidecar_json = serde_json::to_string_pretty(sidecar)
        .map_err(|e| MarketError::BadManifest(e.to_string()))?;
    std::fs::write(staging.join(".tenon-market.json"), sidecar_json)?;
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(&staging, &target)?;
    Ok(target)
}

/// 读取已装技能的 sidecar（非市场条目返回 None）。
pub fn read_sidecar(skill_dir: &Path) -> Option<MarketSidecar> {
    let text = std::fs::read_to_string(skill_dir.join(".tenon-market.json")).ok()?;
    serde_json::from_str(&text).ok()
}
