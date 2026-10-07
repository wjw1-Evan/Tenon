//! 官方静态 registry 客户端（设计方案 §12.5 / §13 / 附录 C Q1）：
//!
//! - **manifest 规范（§13.1 YAML）**：id / version / runtime / permissions /
//!   provides（languages、lsp、formatters、linters）/ requires / signature；
//! - **签名清单**：静态 registry（GitHub Pages 起步）列出条目
//!   {id, version, sha256, signature, url}；客户端 ed25519 验签 + SHA-256 校验；
//! - **检索 / 安装 / 权限 diff（§13.2）**：检索 → 展示权限 diff（相对已装
//!   版本新增权限高亮）→ 用户确认（D 级）→ 签名校验 + 版本锁定 → 安装；
//! - **保留字防 typosquatting（§12.5）**：`official.*` 前缀仅官方签名条目可用。
//!
//! v1.145 起另承载社区通道：GitHub 技能与插件市场客户端（[`market`]，§13.5）；
//! 本文件签名清单能力保留待官方通道启用。

pub mod market;

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use sha2::Digest;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("manifest 解析失败: {0}")]
    Parse(String),
    #[error("manifest 校验失败: {0}")]
    Invalid(String),
    #[error("签名校验失败（§12.5 供应链）")]
    BadSignature,
    #[error("SHA-256 校验失败")]
    BadChecksum,
    #[error("下载失败: {0}")]
    Download(String),
    #[error("保留字 id（防 typosquatting）: {0}")]
    ReservedId(String),
}

pub type Result<T> = std::result::Result<T, RegistryError>;

/// 插件 manifest（§13.1 YAML 规范）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginManifest {
    /// 全局唯一 id（官方条目 `official.<name>`；保留字校验）。
    pub id: String,
    pub version: String,
    /// external（外部进程 / stdio JSON-RPC）| wasm
    pub runtime: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub provides: Provides,
    #[serde(default)]
    pub requires: Requires,
    /// ed25519 签名（对去除 signature 字段后的规范 YAML 字节签名，hex）。
    #[serde(default)]
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Provides {
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub lsp: Option<LspServer>,
    #[serde(default)]
    pub formatters: Vec<String>,
    #[serde(default)]
    pub linters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LspServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Requires {
    #[serde(default)]
    pub runtime: Option<RuntimeReq>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeReq {
    pub node: Option<String>,
}

/// 静态 registry 索引条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegistryEntry {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub signature: String,
    pub url: String,
    /// 摘要（检索展示）。
    #[serde(default)]
    pub description: String,
}

/// 静态 registry 索引（GitHub Pages 起步的静态 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RegistryIndex {
    #[serde(default)]
    pub plugins: Vec<RegistryEntry>,
}

/// 权限 diff（§13.2 安装时展示；新增权限高亮）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PermissionDiff {
    /// 相对已装版本新增（高亮）
    pub added: Vec<String>,
    /// 已移除
    pub removed: Vec<String>,
    /// 保持不变
    pub unchanged: Vec<String>,
}

/// 保留字（§12.5 防 typosquatting）：`official.*` 前缀仅官方签名条目可用。
pub const RESERVED_PREFIXES: [&str; 2] = ["official.", "tenon."];

pub fn is_reserved_id(id: &str) -> bool {
    RESERVED_PREFIXES.iter().any(|p| id.starts_with(p))
}

/// 解析 manifest YAML 并做结构校验。
pub fn parse_manifest(yaml: &str) -> Result<PluginManifest> {
    let m: PluginManifest =
        serde_yaml::from_str(yaml).map_err(|e| RegistryError::Parse(e.to_string()))?;
    validate(&m)?;
    Ok(m)
}

/// 权限串白名单：`<fs|net> [.资源]+ :<scope>[:<子scope>]*`——
/// 如 `fs.read:project`、`net:registry:pypi`。此前「含冒号即通过」会让
/// `admin:all` 之类任意串绕过校验（权限是安装审批 UI 高亮的数据源）。
fn is_valid_permission(perm: &str) -> bool {
    let mut parts = perm.split(':');
    let Some(resource) = parts.next() else {
        return false;
    };
    let resource_ok = resource == "net" || resource.starts_with("fs.");
    let scopes: Vec<&str> = parts.collect();
    let scope_ok = !scopes.is_empty()
        && scopes.iter().all(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '*' || c == '_')
        });
    let resource_chars_ok = resource
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    resource_ok && resource_chars_ok && scope_ok
}

fn validate(m: &PluginManifest) -> Result<()> {
    if m.id.is_empty() || !m.id.contains('.') {
        return Err(RegistryError::Invalid("id 须为 <scope>.<name> 形式".into()));
    }
    if m.version.split('.').count() != 3 {
        return Err(RegistryError::Invalid(format!(
            "version 须为 semver: {}",
            m.version
        )));
    }
    if !matches!(m.runtime.as_str(), "external" | "wasm") {
        return Err(RegistryError::Invalid(format!(
            "未知 runtime: {}",
            m.runtime
        )));
    }
    for perm in &m.permissions {
        if !is_valid_permission(perm) {
            return Err(RegistryError::Invalid(format!("未知权限: {perm}")));
        }
    }
    Ok(())
}

/// 安装前检查（§13.2 D 级确认前调用）：保留字 `official.*` / `tenon.*`
/// 仅当 manifest 带有效官方签名时可用（防 typosquatting，§12.5）。
pub fn pre_install_check(m: &PluginManifest, official_signature_valid: bool) -> Result<()> {
    if is_reserved_id(&m.id) && !official_signature_valid {
        return Err(RegistryError::ReservedId(m.id.clone()));
    }
    Ok(())
}

/// 签名校验：签名 = sign(私钥, sha256_hex(规范 YAML))；
/// 「规范 YAML」= 原文去掉 `signature:` 行后的内容（签名前先签后填）。
pub fn verify_manifest_signature(
    manifest_yaml: &str,
    signature_hex: &str,
    public_key_hex: &str,
) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let body = strip_signature_line(manifest_yaml);
    let digest = sha256_hex(body.as_bytes());
    let Ok(pk_bytes) = hex::decode(public_key_hex) else {
        return false;
    };
    let Ok(pk_arr): std::result::Result<[u8; 32], _> = pk_bytes.try_into() else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk_arr) else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(signature_hex) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(&sig_bytes) else {
        return false;
    };
    vk.verify(digest.as_bytes(), &sig).is_ok()
}

/// 静态 index 中按条目校验（签名对 sha256 hex 字节，与 Laya 同约定）。
pub fn verify_entry(entry: &RegistryEntry, public_key_hex: &str) -> Result<()> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    // 开发模式（显式全零占位公钥，无任何密钥配置）：跳过签名（无钥可验），
    // 仅要求 sha256 完整性字段存在（下载后仍强制 SHA-256 校验）。
    // 空串 / 错长公钥 fail closed——空串对 all() 是 vacuous true，会静默
    // 进入免验签分支。
    // 保留字条目的官方签名约束由 pre_install_check 单独把关（§12.5）；
    // 正式发布注入真实公钥后，签名校验无条件强制。
    if !public_key_hex.is_empty() && public_key_hex.bytes().all(|b| b == b'0') {
        if entry.sha256.len() != 64 {
            return Err(RegistryError::Invalid("条目缺 sha256".into()));
        }
        return Ok(());
    }
    let (Ok(pk_bytes), Ok(sig_bytes)) =
        (hex::decode(public_key_hex), hex::decode(&entry.signature))
    else {
        return Err(RegistryError::BadSignature);
    };
    let Ok(pk_arr): std::result::Result<[u8; 32], _> = pk_bytes.try_into() else {
        return Err(RegistryError::BadSignature);
    };
    let (Ok(vk), Ok(sig)) = (
        VerifyingKey::from_bytes(&pk_arr),
        Signature::from_slice(&sig_bytes),
    ) else {
        return Err(RegistryError::BadSignature);
    };
    // 签名对象 = sha256 hex 字符串字节（与 Laya 验签同约定；对 raw 解码字节
    // 验签会让按约定签出的官方条目全部 BadSignature）
    if vk.verify(entry.sha256.as_bytes(), &sig).is_err() {
        return Err(RegistryError::BadSignature);
    }
    Ok(())
}

fn strip_signature_line(yaml: &str) -> String {
    yaml.lines()
        .filter(|l| !l.trim_start().starts_with("signature:"))
        .map(|l| format!("{l}\n"))
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    hex::encode(digest)
}

/// 权限 diff（§13.2：相对已装版本，新增高亮）。
pub fn permission_diff(old: &[String], new: &[String]) -> PermissionDiff {
    let old_set: BTreeSet<&String> = old.iter().collect();
    let new_set: BTreeSet<&String> = new.iter().collect();
    PermissionDiff {
        added: new_set.difference(&old_set).map(|s| (*s).clone()).collect(),
        removed: old_set.difference(&new_set).map(|s| (*s).clone()).collect(),
        unchanged: old_set
            .intersection(&new_set)
            .map(|s| (*s).clone())
            .collect(),
    }
}

/// 安装计划（下载 + 校验所需字段；与 laya 同约定：签名对 sha256 hex 字节）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPlan {
    pub version: u32,
    pub sha256: String,
    pub signature: String,
    pub url: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// 公钥解析顺序：`TENON_REGISTRY_PUBLIC_KEY` / `TENON_LAYA_PUBLIC_KEY` 环境变量
/// → `~/.tenon/keys/signing.pub` → None（开发模式）。
pub fn load_public_key() -> Option<String> {
    for var in ["TENON_REGISTRY_PUBLIC_KEY", "TENON_LAYA_PUBLIC_KEY"] {
        if let Ok(k) = std::env::var(var) {
            if !k.is_empty() {
                return Some(k);
            }
        }
    }
    let path = std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".tenon/keys/signing.pub"));
    if let Some(path) = path {
        if let Ok(k) = std::fs::read_to_string(&path) {
            let k = k.trim().to_string();
            if !k.is_empty() {
                return Some(k);
            }
        }
    }
    None
}

/// 下载条目并校验 SHA-256。
pub async fn download_entry(plan: &InstallPlan) -> Result<Vec<u8>> {
    let resp = reqwest::Client::new()
        .get(&plan.url)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| RegistryError::Download(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(RegistryError::Download(format!("HTTP {}", resp.status())));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| RegistryError::Download(e.to_string()))?
        .to_vec();
    if sha256_hex(&bytes) != plan.sha256 {
        return Err(RegistryError::BadChecksum);
    }
    Ok(bytes)
}

/// 从静态 index 检索（简单子串匹配 id / description）。
pub fn search_index<'a>(index: &'a RegistryIndex, query: &str) -> Vec<&'a RegistryEntry> {
    let q = query.to_lowercase();
    index
        .plugins
        .iter()
        .filter(|e| e.id.to_lowercase().contains(&q) || e.description.to_lowercase().contains(&q))
        .collect()
}

/// 拉取静态 index。
pub async fn fetch_index(url: Option<&str>) -> Result<RegistryIndex> {
    let url = url.unwrap_or("https://tenonide.dev/registry/plugins.json");
    let resp = reqwest::Client::new()
        .get(url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| RegistryError::Download(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(RegistryError::Download(format!(
            "registry HTTP {}",
            resp.status()
        )));
    }
    resp.json::<RegistryIndex>()
        .await
        .map_err(|e| RegistryError::Parse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MANIFEST: &str = r#"id: official.python
version: 1.2.0
runtime: external
permissions:
  - fs.read:project
  - net:registry:pypi
provides:
  languages: [python]
  lsp:
    command: pyright-langserver
    args: ["--stdio"]
  formatters: [ruff]
  linters: [ruff]
requires:
  runtime:
    node: ">=18"
signature: ""
"#;

    #[test]
    fn parses_design_sample_manifest() {
        let m = parse_manifest(SAMPLE_MANIFEST).unwrap();
        assert_eq!(m.id, "official.python");
        assert_eq!(m.version, "1.2.0");
        assert_eq!(m.runtime, "external");
        assert_eq!(m.permissions, vec!["fs.read:project", "net:registry:pypi"]);
        assert_eq!(m.provides.languages, vec!["python"]);
        let lsp = m.provides.lsp.unwrap();
        assert_eq!(lsp.command, "pyright-langserver");
        assert!(m.requires.runtime.unwrap().node.as_deref() == Some(">=18"));
    }

    #[test]
    fn rejects_invalid_manifests() {
        // 缺 scope
        assert!(parse_manifest("id: python\nversion: 1.0.0\nruntime: external\n").is_err());
        // 非 semver
        assert!(parse_manifest("id: a.b\nversion: 1.0\nruntime: external\n").is_err());
        // 未知 runtime
        assert!(parse_manifest("id: a.b\nversion: 1.0.0\nruntime: magic\n").is_err());
        // 保留字：解析允许，安装期拦截
        let reserved =
            parse_manifest("id: official.python\nversion: 1.0.0\nruntime: external\n").unwrap();
        assert!(matches!(
            pre_install_check(&reserved, false),
            Err(RegistryError::ReservedId(_))
        ));
        assert!(pre_install_check(&reserved, true).is_ok());
    }

    #[test]
    fn signature_verifies_against_stripped_body() {
        use ed25519_dalek::Signer;
        let sk = ed25519_dalek::SigningKey::from_bytes(&[11u8; 32]);
        let body = strip_signature_line(SAMPLE_MANIFEST);
        let digest = sha256_hex(body.as_bytes());
        let sig = hex::encode(sk.sign(digest.as_bytes()).to_bytes());
        let pk = hex::encode(sk.verifying_key().to_bytes());
        assert!(verify_manifest_signature(SAMPLE_MANIFEST, &sig, &pk));
        // 篡改 manifest 内容 → 校验失败
        let tampered = SAMPLE_MANIFEST.replace("1.2.0", "9.9.9");
        assert!(!verify_manifest_signature(&tampered, &sig, &pk));
    }

    #[test]
    fn permission_diff_highlights_additions() {
        let diff = permission_diff(
            &["fs.read:project".into()],
            &["fs.read:project".into(), "net:registry:pypi".into()],
        );
        assert_eq!(diff.added, vec!["net:registry:pypi"]);
        assert!(diff.removed.is_empty());
        assert_eq!(diff.unchanged, vec!["fs.read:project"]);
    }

    #[test]
    fn search_filters_by_substring() {
        let index = RegistryIndex {
            plugins: vec![
                RegistryEntry {
                    id: "official.python".into(),
                    version: "1.0.0".into(),
                    sha256: "a".repeat(64),
                    signature: "x".into(),
                    url: "u1".into(),
                    description: "Python language pack".into(),
                },
                RegistryEntry {
                    id: "community.mdfmt".into(),
                    version: "0.3.0".into(),
                    sha256: "b".repeat(64),
                    signature: "y".into(),
                    url: "u2".into(),
                    description: "Markdown formatter".into(),
                },
            ],
        };
        let hits = search_index(&index, "python");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "official.python");
    }

    #[test]
    fn entry_verification_with_real_key() {
        use ed25519_dalek::Signer;
        let sk = ed25519_dalek::SigningKey::from_bytes(&[21u8; 32]);
        let pk = hex::encode(sk.verifying_key().to_bytes());
        let mut e = RegistryEntry {
            id: "community.x".into(),
            version: "1.0.0".into(),
            sha256: sha256_hex(b"pkg"),
            signature: String::new(),
            url: "u".into(),
            description: String::new(),
        };
        // 约定：对 sha256 hex 字符串字节签名（与 Laya 同口径）
        e.signature = hex::encode(sk.sign(e.sha256.as_bytes()).to_bytes());
        assert!(verify_entry(&e, &pk).is_ok());
        e.sha256 = sha256_hex(b"tampered");
        assert!(matches!(
            verify_entry(&e, &pk),
            Err(RegistryError::BadSignature)
        ));
        // 空 / 错长公钥 fail closed（空串不得 vacuously 进入开发模式）
        assert!(matches!(
            verify_entry(&e, ""),
            Err(RegistryError::BadSignature)
        ));
        assert!(matches!(
            verify_entry(&e, "abcd"),
            Err(RegistryError::BadSignature)
        ));
    }

    #[test]
    fn reserved_ids_detected() {
        assert!(is_reserved_id("official.python"));
        assert!(is_reserved_id("official.typescript"));
        assert!(!is_reserved_id("community.python"));
        assert!(!is_reserved_id("python"));
    }

    #[test]
    fn sha256_hex_produces_correct_length() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash.len(), 64);
        // Known SHA-256 of "hello"
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_hex_empty_input() {
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn pre_install_check_rejects_unofficial() {
        let m = parse_manifest(SAMPLE_MANIFEST).unwrap();
        assert!(pre_install_check(&m, false).is_err());
        assert!(pre_install_check(&m, true).is_ok());
    }

    #[test]
    fn permission_diff_detects_new_permissions() {
        let diff = PermissionDiff {
            added: vec!["net:new:api".into()],
            removed: vec![],
            unchanged: vec!["fs.read:project".into()],
        };
        assert_eq!(diff.added.len(), 1);
        assert!(diff.removed.is_empty());
    }
}
