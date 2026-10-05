//! Laya 本地决策模型（设计方案 §9.8）：
//! 产品自管的小型本地分类模型，只做结构化判定——选项分类（choice）/
//! 量表打分（score）/ 布尔判断（bool）三类原语；本地 CPU 推理（~30ms 级）、
//! 零 token 成本；不生成代码、不做开放问答。
//!
//! 边界（铁律）：
//! - 判定输出只用于排序、提示、预筛，绝不直接产生动作、绝不放宽只读开关或黑名单；
//! - 置信度未校准，仅作排序参考；
//! - 模型输入含仓库文本（不可信数据）→ 判定结果同样按数据处理；
//! - 整体可回退：未下载 / 加载失败 / 单次推理超时（默认 200ms）→ 对应
//!   集成点回退现状，任何功能不阻塞。

pub mod model;
pub mod primitives;
pub mod registry;
pub mod runtime;

pub use primitives::{DecisionKind, Feature, IntentLabel, LayaOutcome};
pub use runtime::LayaRuntime;

/// 静态 registry 默认地址（附录 C Q1：静态清单 + 对象存储 / CDN，
/// GitHub Pages 起步）。
pub const DEFAULT_REGISTRY_URL: &str = "https://tenonide.dev/registry/laya.json";

/// 产品发布签名公钥（hex ed25519）。解析顺序：
/// 1. 环境变量 `TENON_LAYA_PUBLIC_KEY`（CI / 部署覆盖）；
/// 2. `~/.tenon/keys/signing.pub`（`tenon-daemon --generate-keys` 产物，开发流）；
/// 3. 编译期内嵌公钥（正式发布以 `TENON_RELEASE_PUBKEY` 构建时注入；无则 None = 开发模式）。
pub fn release_public_key() -> Option<String> {
    if let Ok(k) = std::env::var("TENON_LAYA_PUBLIC_KEY") {
        if !k.is_empty() {
            return Some(k);
        }
    }
    let path = tenon_config::Config::data_dir().join("keys/signing.pub");
    if let Ok(k) = std::fs::read_to_string(&path) {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Some(k);
        }
    }
    match std::env::var("TENON_RELEASE_PUBKEY") {
        Ok(k) if !k.is_empty() => Some(k),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LayaError {
    #[error("模型未安装")]
    NotInstalled,
    #[error("模型加载失败: {0}")]
    Load(String),
    #[error("签名校验失败（§9.8 供应链）")]
    BadSignature,
    #[error("SHA-256 校验失败")]
    BadChecksum,
    #[error("registry 清单解析失败: {0}")]
    Manifest(String),
    #[error("下载失败: {0}")]
    Download(String),
    #[error("推理超时（> {0}ms，回退现状）")]
    Timeout(u64),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, LayaError>;

/// SHA-256 摘要（hex）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    hex::encode(digest)
}

/// ed25519 签名校验：签名 = sign(私钥, model_sha256_bytes)。
pub fn verify_signature(model_sha256_hex: &str, signature_hex: &str, public_key_hex: &str) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let (Ok(pk_bytes), Ok(sig_bytes)) = (hex::decode(public_key_hex), hex::decode(signature_hex))
    else {
        return false;
    };
    let (Ok(verifying), Ok(sig)) = (
        VerifyingKey::from_bytes(&pk_bytes.try_into().unwrap_or([0u8; 32])),
        Signature::from_slice(&sig_bytes),
    ) else {
        return false;
    };
    // 签名对象 = sha256 的 ASCII hex 字符串字节（展示值即签名值）
    verifying.verify(model_sha256_hex.as_bytes(), &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_roundtrip_with_generated_keypair() {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let model_hash = sha256_hex(b"model-bytes");
        let sig = sk.sign(model_hash.as_bytes());
        let pk = hex::encode(sk.verifying_key().to_bytes());
        assert!(verify_signature(
            &model_hash,
            &hex::encode(sig.to_bytes()),
            &pk
        ));
        // 篡改 hash → 校验失败
        assert!(!verify_signature(
            &sha256_hex(b"tampered"),
            &hex::encode(sig.to_bytes()),
            &pk
        ));
    }
}
