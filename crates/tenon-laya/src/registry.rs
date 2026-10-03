//! 静态 registry 分发（附录 C Q1 + §9.8 分发生命周期）：
//! 签名清单（GitHub Pages 起步的静态 JSON）→ 签名校验（ed25519）→
//! SHA-256 展示 → **一次 D 级审批** → 安装入 `~/.tenon/models/laya/`。
//!
//! 审批由调用方（daemon 审批卡）执行：本模块产出 `InstallPlan` 供审批卡展示，
//! 拿到批准后才调用 [`install_model`]。

use serde::{Deserialize, Serialize};

use crate::{verify_signature, LayaError, Result, DEFAULT_REGISTRY_URL};

/// registry 清单（静态 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryManifest {
    pub laya: ModelEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// 版本锁定（§12.5）。
    pub version: u32,
    /// 模型文件 SHA-256（hex，审批卡展示，§9.8）。
    pub sha256: String,
    /// ed25519 签名（对 sha256 hex 字节签名，hex）。
    pub signature: String,
    /// 模型文件下载地址。
    pub url: String,
}

/// 安装计划（D 级审批卡内容，§9.8：与 §8.4 运行时下载同款）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPlan {
    pub version: u32,
    pub sha256: String,
    pub url: String,
    pub size_bytes: Option<u64>,
}

/// 拉取静态 registry 清单。
pub async fn fetch_manifest(registry_url: Option<&str>) -> Result<RegistryManifest> {
    let url = registry_url.unwrap_or(DEFAULT_REGISTRY_URL);
    let resp = reqwest::Client::new()
        .get(url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| LayaError::Download(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(LayaError::Download(format!(
            "registry HTTP {}",
            resp.status()
        )));
    }
    let manifest: RegistryManifest = resp
        .json()
        .await
        .map_err(|e| LayaError::Manifest(e.to_string()))?;
    Ok(manifest)
}

/// 校验清单签名 + 生成安装计划（未过 D 级审批前不下载模型体）。
///
/// 公钥解析（§9.8）：环境变量 `TENON_LAYA_PUBLIC_KEY` → `~/.tenon/keys/signing.pub`
/// （`--generate-keys` 产物）→ None（开发模式：仅校验清单格式，正式发布强制验签）。
pub fn plan_install(manifest: &RegistryManifest) -> Result<InstallPlan> {
    let entry = &manifest.laya;
    match crate::release_public_key() {
        None => {
            if entry.signature.is_empty() || entry.sha256.len() != 64 {
                return Err(LayaError::Manifest("清单缺签名或 sha256".into()));
            }
        }
        Some(pk) => {
            if !verify_signature(&entry.sha256, &entry.signature, &pk) {
                return Err(LayaError::BadSignature);
            }
        }
    }
    Ok(InstallPlan {
        version: entry.version,
        sha256: entry.sha256.clone(),
        url: entry.url.clone(),
        size_bytes: None,
    })
}

/// 下载模型体并校验 SHA-256（**须已过 D 级审批**；返回待安装字节）。
pub async fn download_model(plan: &InstallPlan) -> Result<Vec<u8>> {
    let resp = reqwest::Client::new()
        .get(&plan.url)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| LayaError::Download(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(LayaError::Download(format!("模型 HTTP {}", resp.status())));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| LayaError::Download(e.to_string()))?
        .to_vec();
    if crate::sha256_hex(&bytes) != plan.sha256 {
        return Err(LayaError::BadChecksum);
    }
    Ok(bytes)
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
}
