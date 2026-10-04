//! 更新执行器（v1.86 / §4.2）：签名清单 → 平台选择 → 哈希验签 → 原子 staging。
//!
//! daemon 不运行安装脚本，也不做运行中原地热替换。已验证产物写入 staging，
//! 下次启动绑定端口前替换当前可执行文件；替换失败保留旧版并 fail open 运行。

use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("更新公钥未配置")]
    PublicKeyMissing,
    #[error("公钥格式非法")]
    BadPublicKey,
    #[error("清单解析失败: {0}")]
    Manifest(String),
    #[error("当前平台没有可用更新")]
    NoPlatform,
    #[error("版本必须大于当前版本")]
    NotNewer,
    #[error("版本格式非法")]
    BadVersion,
    #[error("SHA-256 校验失败")]
    BadChecksum,
    #[error("更新签名校验失败")]
    BadSignature,
    #[error("下载失败: {0}")]
    Download(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub platforms: BTreeMap<String, PlatformUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformUpdate {
    pub url: String,
    pub sha256: String,
    /// ed25519 签名对象是 SHA-256 hex 的 ASCII 字节。
    pub signature: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedUpdate {
    pub version: String,
    pub target: String,
    pub sha256: String,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub channel: String,
    pub current_version: String,
    pub last_check_at: Option<String>,
    pub last_error: Option<String>,
    pub staged: Option<StagedUpdate>,
}

const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

pub fn current_target() -> String {
    match std::env::consts::OS {
        "macos" => format!("{}-apple-darwin", std::env::consts::ARCH),
        "linux" => format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
        "windows" => format!("{}-pc-windows-msvc", std::env::consts::ARCH),
        _ => format!("{}-unknown", std::env::consts::ARCH),
    }
}

pub fn version_is_newer(candidate: &str, current: &str) -> Result<bool> {
    let parse = |v: &str| -> Result<(u64, u64, u64)> {
        let parts: Vec<&str> = v.trim().split('.').collect();
        if parts.len() != 3 {
            return Err(UpdateError::BadVersion);
        }
        Ok((
            parts[0].parse().map_err(|_| UpdateError::BadVersion)?,
            parts[1].parse().map_err(|_| UpdateError::BadVersion)?,
            parts[2].parse().map_err(|_| UpdateError::BadVersion)?,
        ))
    };
    Ok(parse(candidate)? > parse(current)?)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

fn verify_signature(artifact_sha256_hex: &str, signature_hex: &str, public_key_hex: &str) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let (Ok(pk), Ok(sig)) = (hex::decode(public_key_hex), hex::decode(signature_hex)) else {
        return false;
    };
    let (Ok(verifying), Ok(sig)) = (
        VerifyingKey::from_bytes(&pk.try_into().unwrap_or([0u8; 32])),
        Signature::from_slice(&sig),
    ) else {
        return false;
    };
    verifying
        .verify(artifact_sha256_hex.as_bytes(), &sig)
        .is_ok()
}

async fn fetch_manifest(url: &str) -> Result<UpdateManifest> {
    let response = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| UpdateError::Download(e.to_string()))?;
    if !response.status().is_success() {
        return Err(UpdateError::Download(format!(
            "manifest HTTP {}",
            response.status()
        )));
    }
    response
        .json()
        .await
        .map_err(|e| UpdateError::Manifest(e.to_string()))
}

async fn download_limited(url: &str, expected_size: Option<u64>) -> Result<Vec<u8>> {
    let response = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| UpdateError::Download(e.to_string()))?;
    if !response.status().is_success() {
        return Err(UpdateError::Download(format!(
            "artifact HTTP {}",
            response.status()
        )));
    }
    let actual_size = response.content_length();
    if let Some(size) = actual_size {
        if size > MAX_ARTIFACT_BYTES {
            return Err(UpdateError::Download("artifact 超过 512MB 上限".into()));
        }
        if let Some(expected) = expected_size {
            if size != expected {
                return Err(UpdateError::Download("Content-Length 与清单不一致".into()));
            }
        }
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| UpdateError::Download(e.to_string()))?;
        if bytes.len() as u64 + chunk.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err(UpdateError::Download("artifact 超过 512MB 上限".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    if let Some(expected) = expected_size {
        if bytes.len() as u64 != expected {
            return Err(UpdateError::Download("artifact 大小与清单不一致".into()));
        }
    }
    Ok(bytes)
}

/// 检查并 staging 更新。函数只负责验证与原子落盘；应用交给启动路径。
pub async fn stage_update(
    manifest_url: &str,
    public_key_hex: &str,
    current_version: &str,
    target: &str,
    staging_dir: &Path,
) -> Result<StagedUpdate> {
    if public_key_hex.trim().is_empty() {
        return Err(UpdateError::PublicKeyMissing);
    }
    if hex::decode(public_key_hex.trim())
        .map_err(|_| UpdateError::BadPublicKey)?
        .len()
        != 32
    {
        return Err(UpdateError::BadPublicKey);
    }
    let manifest = fetch_manifest(manifest_url).await?;
    if !version_is_newer(&manifest.version, current_version)? {
        return Err(UpdateError::NotNewer);
    }
    let platform = manifest
        .platforms
        .get(target)
        .ok_or(UpdateError::NoPlatform)?;
    if platform.sha256.len() != 64 || hex::decode(&platform.sha256).is_err() {
        return Err(UpdateError::BadChecksum);
    }
    if !verify_signature(&platform.sha256, &platform.signature, public_key_hex.trim()) {
        return Err(UpdateError::BadSignature);
    }
    let bytes = download_limited(&platform.url, platform.size_bytes).await?;
    let actual_hash = sha256_hex(&bytes);
    if actual_hash != platform.sha256.to_lowercase() {
        return Err(UpdateError::BadChecksum);
    }

    std::fs::create_dir_all(staging_dir)?;
    let destination = staging_dir.join(format!(
        "tenon-{}-{}-{actual_hash}",
        manifest.version, target
    ));
    let temp = staging_dir.join(format!(
        ".{}.tmp",
        destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("update")
    ));
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&temp, &destination)?;
    Ok(StagedUpdate {
        version: manifest.version,
        target: target.to_string(),
        sha256: actual_hash,
        path: destination.to_string_lossy().into_owned(),
        size_bytes: bytes.len() as u64,
    })
}

/// 启动前应用 staged 更新：同文件系统 rename 覆盖旧可执行文件。
/// Windows 上运行中替换可能失败；调用方保留 daemon 并继续旧版。
pub fn apply_staged_update(staged_path: &Path, current_exe: &Path) -> Result<()> {
    if !staged_path.is_file() {
        return Err(
            std::io::Error::new(std::io::ErrorKind::NotFound, "staged update missing").into(),
        );
    }
    let bytes = std::fs::read(staged_path)?;
    let file_hash = sha256_hex(&bytes);
    let expected_hash = staged_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.get(name.len().saturating_sub(64)..))
        .ok_or(UpdateError::BadChecksum)?;
    if expected_hash != file_hash {
        return Err(UpdateError::BadChecksum);
    }
    std::fs::rename(staged_path, current_exe)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(current_exe, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// 扫描当前平台 staged 更新；文件名内嵌哈希，选择最大新版本。
pub fn find_staged_update(
    staging_dir: &Path,
    target: &str,
    current_version: &str,
) -> Result<Option<StagedUpdate>> {
    let mut best: Option<(String, PathBuf, String)> = None;
    for entry in std::fs::read_dir(staging_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((version, hash)) = name
            .strip_prefix("tenon-")
            .and_then(|rest| rest.rsplit_once('-'))
            .and_then(|(before, hash)| {
                before
                    .strip_suffix(&format!("-{target}"))
                    .map(|version| (version, hash))
            })
        else {
            continue;
        };
        if hash.len() != 64 || hex::decode(hash).is_err() {
            continue;
        }
        if !version_is_newer(version, current_version)? {
            continue;
        }
        if best
            .as_ref()
            .map(|(best_version, _, _)| version_is_newer(version, best_version).unwrap_or(false))
            .unwrap_or(true)
        {
            best = Some((version.to_string(), path.clone(), hash.to_string()));
        }
    }
    Ok(best.map(|(version, path, sha256)| StagedUpdate {
        size_bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        version,
        path: path.to_string_lossy().into_owned(),
        sha256,
        target: target.to_string(),
    }))
}

/// 用户确认下次启动应用；请求文件只保存 path + 哈希，应用前再次重算。
pub fn mark_staged_update(staged: &StagedUpdate, staging_dir: &Path) -> Result<()> {
    let path = staging_dir.join(".apply");
    std::fs::write(
        &path,
        serde_json::to_vec(staged).map_err(|e| UpdateError::Manifest(e.to_string()))?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// 启动路径取出应用请求（一次性）；随后仍会对 artifact 做 SHA-256 重验。
pub fn take_apply_request(staging_dir: &Path) -> Result<Option<StagedUpdate>> {
    let path = staging_dir.join(".apply");
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let _ = std::fs::remove_file(&path);
    let staged: StagedUpdate =
        serde_json::from_slice(&bytes).map_err(|e| UpdateError::Manifest(e.to_string()))?;
    Ok(Some(staged))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_platform(bytes: &[u8]) -> (PlatformUpdate, String) {
        use ed25519_dalek::Signer;
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let hash = sha256_hex(bytes);
        (
            PlatformUpdate {
                url: "http://127.0.0.1/update".into(),
                sha256: hash.clone(),
                signature: hex::encode(key.sign(hash.as_bytes()).to_bytes()),
                size_bytes: Some(bytes.len() as u64),
            },
            hex::encode(key.verifying_key().to_bytes()),
        )
    }

    #[test]
    fn semantic_version_requires_newer_release() {
        assert!(version_is_newer("0.2.0", "0.1.0").unwrap());
        assert!(!version_is_newer("0.1.0", "0.1.0").unwrap());
        assert!(!version_is_newer("0.1.0", "0.2.0").unwrap());
        assert!(matches!(
            version_is_newer("x", "0.1.0"),
            Err(UpdateError::BadVersion)
        ));
    }

    #[test]
    fn signature_rejects_wrong_hash_or_key() {
        let (platform, public_key) = signed_platform(b"correct");
        assert!(verify_signature(
            &platform.sha256,
            &platform.signature,
            &public_key
        ));
        assert!(!verify_signature(
            &sha256_hex(b"wrong"),
            &platform.signature,
            &public_key
        ));
        assert!(!verify_signature(
            &platform.sha256,
            &platform.signature,
            &hex::encode([1; 32])
        ));
    }

    #[tokio::test]
    async fn missing_public_key_fails_closed_before_network() {
        let error = stage_update(
            "http://127.0.0.1:1/manifest.json",
            "",
            "0.1.0",
            "test-target",
            Path::new("/nonexistent-staging"),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, UpdateError::PublicKeyMissing));
    }

    #[tokio::test]
    async fn stages_signed_artifact_and_applies_after_hash_recheck() {
        let bytes = b"#!/bin/sh\ntenon-next\n";
        let (mut platform, public_key) = signed_platform(bytes);
        let target = current_target();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        platform.url = format!("http://{addr}/update");
        let manifest = UpdateManifest {
            version: "99.0.0".into(),
            platforms: [(target.clone(), platform)].into_iter().collect(),
        };

        let manifest_payload = serde_json::to_vec(&manifest).unwrap();
        let artifact = bytes.to_vec();
        let app =
            axum::Router::new()
                .route(
                    "/manifest.json",
                    axum::routing::get(move || {
                        let body = manifest_payload.clone();
                        async move {
                            axum::Json(serde_json::from_slice::<UpdateManifest>(&body).unwrap())
                        }
                    }),
                )
                .route(
                    "/update",
                    axum::routing::get(move || {
                        let body = artifact.clone();
                        async move { (axum::http::StatusCode::OK, body) }
                    }),
                );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staged");
        let staged = stage_update(
            &format!("http://{addr}/manifest.json"),
            &public_key,
            "0.1.0",
            &target,
            &staging,
        )
        .await
        .unwrap();
        assert_eq!(staged.version, "99.0.0");
        assert_eq!(staged.size_bytes, bytes.len() as u64);

        let found = find_staged_update(&staging, &target, "0.1.0")
            .unwrap()
            .unwrap();
        assert_eq!(found.sha256, staged.sha256);
        let current = dir.path().join("current-daemon");
        std::fs::write(&current, b"old").unwrap();
        apply_staged_update(Path::new(&found.path), &current).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), bytes);
        assert!(!Path::new(&found.path).exists());
    }

    #[tokio::test]
    async fn tampered_staged_artifact_is_rejected() {
        let bytes = b"valid";
        let (mut platform, public_key) = signed_platform(bytes);
        let target = current_target();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        platform.url = format!("http://{addr}/update");
        let manifest = UpdateManifest {
            version: "99.0.0".into(),
            platforms: [(target.clone(), platform)].into_iter().collect(),
        };
        let manifest_payload = serde_json::to_vec(&manifest).unwrap();
        let artifact = bytes.to_vec();
        let app =
            axum::Router::new()
                .route(
                    "/manifest.json",
                    axum::routing::get(move || {
                        let body = manifest_payload.clone();
                        async move {
                            axum::Json(serde_json::from_slice::<UpdateManifest>(&body).unwrap())
                        }
                    }),
                )
                .route(
                    "/update",
                    axum::routing::get(move || {
                        let body = artifact.clone();
                        async move { (axum::http::StatusCode::OK, body) }
                    }),
                );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        stage_update(
            &format!("http://{addr}/manifest.json"),
            &public_key,
            "0.1.0",
            &target,
            dir.path(),
        )
        .await
        .unwrap();
        let staged = find_staged_update(dir.path(), &target, "0.1.0")
            .unwrap()
            .unwrap();
        std::fs::write(&staged.path, b"tampered").unwrap();
        let current = dir.path().join("current-daemon");
        std::fs::write(&current, b"old").unwrap();
        let error = apply_staged_update(Path::new(&staged.path), &current).unwrap_err();
        assert!(matches!(error, UpdateError::BadChecksum));
        assert_eq!(std::fs::read(&current).unwrap(), b"old");
    }

    #[tokio::test]
    async fn apply_request_is_one_time_and_verified_again() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_apply_request(dir.path()).unwrap().is_none());
        let staged = StagedUpdate {
            version: "99.0.0".into(),
            target: "test-target".into(),
            sha256: sha256_hex(b"valid"),
            path: dir
                .path()
                .join(format!("tenon-99.0.0-test-target-{}", sha256_hex(b"valid")))
                .to_string_lossy()
                .into_owned(),
            size_bytes: 5,
        };
        std::fs::write(&staged.path, b"valid").unwrap();
        mark_staged_update(&staged, dir.path()).unwrap();
        let taken = take_apply_request(dir.path()).unwrap().unwrap();
        assert_eq!(taken.sha256, staged.sha256);
        assert!(take_apply_request(dir.path()).unwrap().is_none());
    }
}
