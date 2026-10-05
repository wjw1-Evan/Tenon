//! 静态 registry 分发（附录 C Q1 + §9.8 分发生命周期）：
//! 签名清单（多镜像链顺序尝试，v1.102）→ 签名校验（ed25519）→
//! SHA-256 校验 → 按清单镜像列表下载 → 安装入 `~/.tenon/models/laya/` 并热装载。
//!
//! v1.71 起无审批卡：daemon 启动自动执行全链路（产品自管、版本锁定、
//! 签名钉扎的静态资产，推理不出网，非代理动作）；本模块产出 `InstallPlan`
//! 供启动自动下载与手动重下共用。

use serde::{Deserialize, Serialize};

use crate::{verify_signature, LayaError, Result, DEFAULT_REGISTRY_URLS};

/// registry 清单（静态 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryManifest {
    pub laya: ModelEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// 版本锁定（§12.5）。
    pub version: u32,
    /// 模型文件 SHA-256（hex，安装审计展示，§9.8）。
    pub sha256: String,
    /// ed25519 签名（对 sha256 hex 字节签名，hex）。
    pub signature: String,
    /// 模型文件下载地址（主镜像）。
    pub url: String,
    /// 模型文件镜像列表（v1.102，可选；SHA-256 钉扎保证任一镜像字节一致）。
    #[serde(default)]
    pub urls: Vec<String>,
}

impl ModelEntry {
    /// 下载地址全列表：`urls` 非空优先（缺主镜像则前置），否则退回单 `url`。
    pub fn download_urls(&self) -> Vec<String> {
        let mut urls = self.urls.clone();
        if !urls.iter().any(|u| u == &self.url) {
            urls.insert(0, self.url.clone());
        }
        if urls.is_empty() {
            urls.push(self.url.clone());
        }
        urls
    }
}

/// 安装计划（D 级审计内容，§9.8：与 §8.4 运行时下载同款）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPlan {
    pub version: u32,
    pub sha256: String,
    pub url: String,
    /// 模型镜像列表（v1.102）：下载顺序尝试，SHA-256 校验通过即安装。
    #[serde(default)]
    pub urls: Vec<String>,
}

impl InstallPlan {
    /// 下载地址全列表（同 [`ModelEntry::download_urls`] 语义）。
    pub fn download_urls(&self) -> Vec<String> {
        let mut urls = self.urls.clone();
        if !urls.iter().any(|u| u == &self.url) {
            urls.insert(0, self.url.clone());
        }
        if urls.is_empty() {
            urls.push(self.url.clone());
        }
        urls
    }
}

/// 拉取静态 registry 清单：显式地址（测试注入）单点尝试；
/// 未指定则走 [`DEFAULT_REGISTRY_URLS`] 多镜像链，全部失败返回最后错误。
pub async fn fetch_manifest(registry_url: Option<&str>) -> Result<RegistryManifest> {
    let urls: Vec<&str> = match registry_url {
        Some(u) => vec![u],
        None => DEFAULT_REGISTRY_URLS.to_vec(),
    };
    fetch_manifest_from(&urls).await
}

/// 按给定地址列表顺序拉取清单，任一成功即返回。
async fn fetch_manifest_from(urls: &[&str]) -> Result<RegistryManifest> {
    let client = reqwest::Client::new();
    let mut last_err = LayaError::Download("无可用 registry 地址".into());
    for url in urls {
        let resp = match client
            .get(*url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = LayaError::Download(format!("{url}: {e}"));
                continue;
            }
        };
        if !resp.status().is_success() {
            last_err = LayaError::Download(format!("{url}: registry HTTP {}", resp.status()));
            continue;
        }
        match resp.json::<RegistryManifest>().await {
            Ok(m) => return Ok(m),
            Err(e) => last_err = LayaError::Manifest(format!("{url}: {e}")),
        }
    }
    Err(last_err)
}

/// 校验清单签名 + 生成安装计划。
///
/// 公钥解析（§9.8）：环境变量 `TENON_LAYA_PUBLIC_KEY` → `~/.tenon/keys/signing.pub`
/// （`--generate-keys` 产物）→ None（开发模式：仅校验清单格式，正式发布强制验签）。
pub fn plan_install(manifest: &RegistryManifest) -> Result<InstallPlan> {
    plan_install_with_key(manifest, crate::release_public_key().as_deref())
}

/// 同 [`plan_install`]，公钥显式注入（daemon per-instance 覆盖）：不读进程环境——
/// 同进程并行测试下 env 会串扰 §12.5 插件验签链（`TENON_LAYA_PUBLIC_KEY` 是其回退项）。
pub fn plan_install_with_key(
    manifest: &RegistryManifest,
    public_key: Option<&str>,
) -> Result<InstallPlan> {
    let entry = &manifest.laya;
    match public_key {
        None => {
            if entry.signature.is_empty() || entry.sha256.len() != 64 {
                return Err(LayaError::Manifest("清单缺签名或 sha256".into()));
            }
        }
        Some(pk) => {
            if !verify_signature(&entry.sha256, &entry.signature, pk) {
                return Err(LayaError::BadSignature);
            }
        }
    }
    Ok(InstallPlan {
        version: entry.version,
        sha256: entry.sha256.clone(),
        url: entry.url.clone(),
        urls: entry.urls.clone(),
    })
}

/// 按镜像列表顺序下载模型体并校验 SHA-256（v1.102：`urls` 全部失败才报错，
/// 返回最后一次错误；SHA-256 钉扎保证任一镜像字节一致）。
pub async fn download_model(plan: &InstallPlan) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let mut last_err = LayaError::Download("无可用模型镜像".into());
    for url in plan.download_urls() {
        let resp = match client
            .get(&url)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = LayaError::Download(format!("{url}: {e}"));
                continue;
            }
        };
        if !resp.status().is_success() {
            last_err = LayaError::Download(format!("{url}: 模型 HTTP {}", resp.status()));
            continue;
        }
        let bytes = match resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                last_err = LayaError::Download(format!("{url}: {e}"));
                continue;
            }
        };
        if crate::sha256_hex(&bytes) != plan.sha256 {
            last_err = LayaError::BadChecksum;
            continue;
        }
        return Ok(bytes);
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_rejects_bad_signature_with_real_key() {
        std::env::remove_var("TENON_LAYA_PUBLIC_KEY");
        // RELEASE 公钥为占位 → 只查格式
        let m = RegistryManifest {
            laya: ModelEntry {
                version: 1,
                sha256: "a".repeat(64),
                signature: "ff".into(),
                url: "https://example.com/model.json".into(),
                urls: vec![],
            },
        };
        assert!(plan_install(&m).is_ok(), "占位密钥：格式校验通过");

        // 真实密钥环境：签名必须匹配
        use ed25519_dalek::Signer;
        let sk = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let pk = hex::encode(sk.verifying_key().to_bytes());
        std::env::set_var("TENON_LAYA_PUBLIC_KEY", &pk);
        let mut m2 = m.clone();
        m2.laya.signature = hex::encode(sk.sign(m2.laya.sha256.as_bytes()).to_bytes());
        assert!(plan_install(&m2).is_ok());
        // 篡改 sha256 → 签名校验失败
        m2.laya.sha256 = "b".repeat(64);
        assert!(matches!(plan_install(&m2), Err(LayaError::BadSignature)));
        std::env::remove_var("TENON_LAYA_PUBLIC_KEY");
    }

    #[test]
    fn manifest_urls_field_backward_compatible() {
        // 旧清单无 urls → 默认空，退回单 url
        let old = r#"{"laya":{"version":1,"sha256":"a","signature":"s","url":"https://a/m.json"}}"#;
        let m: RegistryManifest = serde_json::from_str(old).unwrap();
        assert!(m.laya.urls.is_empty());
        assert_eq!(m.laya.download_urls(), vec!["https://a/m.json"]);
        // 新清单 urls 缺主镜像 → 主镜像前置；含主镜像 → 原序保留
        let new = r#"{"laya":{"version":1,"sha256":"a","signature":"s","url":"https://a/m.json","urls":["https://b/m.json"]}}"#;
        let m2: RegistryManifest = serde_json::from_str(new).unwrap();
        assert_eq!(
            m2.laya.download_urls(),
            vec!["https://a/m.json", "https://b/m.json"]
        );
    }

    /// 本地静态 HTTP 端点（手写响应，免引服务端依赖）；返回 base URL。
    async fn spawn_status_endpoint(status: u16, body: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let _ = sock.read(&mut [0u8; 4096]).await;
                let head = format!(
                    "HTTP/1.1 {status} T\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
            }
        });
        format!("http://{addr}/x")
    }

    #[tokio::test]
    async fn fetch_manifest_falls_through_to_next_mirror() {
        let bad = spawn_status_endpoint(503, b"{}").await;
        let manifest = serde_json::json!({"laya": {
            "version": 2,
            "sha256": crate::sha256_hex(crate::STARTER_MODEL),
            "signature": "",
            "url": "https://example.com/model.json",
        }})
        .to_string();
        let good_body: &'static [u8] = Box::leak(manifest.into_bytes().into_boxed_slice());
        let good = spawn_status_endpoint(200, good_body).await;
        let m = fetch_manifest_from(&[bad.as_str(), good.as_str()])
            .await
            .unwrap();
        assert_eq!(m.laya.version, 2);
    }

    #[tokio::test]
    async fn fetch_manifest_all_mirrors_down_reports_last_error() {
        let bad1 = spawn_status_endpoint(503, b"{}").await;
        let bad2 = spawn_status_endpoint(404, b"{}").await;
        let err = fetch_manifest_from(&[bad1.as_str(), bad2.as_str()])
            .await
            .unwrap_err();
        assert!(matches!(err, LayaError::Download(_)), "{err}");
        assert!(err.to_string().contains("404"), "最后错误应是末镜像: {err}");
    }

    #[tokio::test]
    async fn download_model_tries_mirrors_until_checksum_passes() {
        let bad = spawn_status_endpoint(503, b"{}").await;
        let model_body: &'static [u8] = Box::leak(crate::STARTER_MODEL.to_vec().into_boxed_slice());
        let good = spawn_status_endpoint(200, model_body).await;
        let plan = InstallPlan {
            version: 2,
            sha256: crate::sha256_hex(crate::STARTER_MODEL),
            url: bad,
            urls: vec![good.clone()],
        };
        let bytes = download_model(&plan).await.unwrap();
        assert_eq!(bytes, crate::STARTER_MODEL);

        // 全镜像失败 → 报最后镜像错误
        let bad2 = spawn_status_endpoint(404, b"{}").await;
        let plan2 = InstallPlan {
            version: 2,
            sha256: crate::sha256_hex(crate::STARTER_MODEL),
            url: bad2,
            urls: vec![],
        };
        assert!(matches!(
            download_model(&plan2).await,
            Err(LayaError::Download(_))
        ));
    }
}
