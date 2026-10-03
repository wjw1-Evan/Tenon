//! 密钥存储（设计方案 §11）：macOS 钥匙串 / 环境变量，不落盘明文。

use std::collections::BTreeMap;
use std::sync::Mutex;

/// 密钥存储抽象：按环境变量名 / 引用名取密钥。
pub trait KeyStore: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, value: &str);
    fn delete(&self, name: &str);
}

/// 环境变量存储（开发 / CI 用）。
pub struct EnvKeyStore;

impl KeyStore for EnvKeyStore {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|s| !s.is_empty())
    }

    fn set(&self, name: &str, value: &str) {
        // 环境变量无法在子进程外修改——仅进程内可见（测试用）
        std::env::set_var(name, value);
    }

    fn delete(&self, name: &str) {
        std::env::remove_var(name);
    }
}

/// 进程内内存存储（测试 / 单元级使用）。
#[derive(Default)]
pub struct MemoryKeyStore {
    map: Mutex<BTreeMap<String, String>>,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyStore for MemoryKeyStore {
    fn get(&self, name: &str) -> Option<String> {
        self.map.lock().expect("keystore lock").get(name).cloned()
    }

    fn set(&self, name: &str, value: &str) {
        self.map
            .lock()
            .expect("keystore lock")
            .insert(name.to_string(), value.to_string());
    }

    fn delete(&self, name: &str) {
        self.map.lock().expect("keystore lock").remove(name);
    }
}

/// macOS 钥匙串存储（`security` CLI；服务名 tenon.keys）。
/// Windows Credential Manager / libsecret 随 M1 三平台工作落地。
pub struct KeychainStore {
    service: String,
}

impl KeychainStore {
    pub fn new() -> Self {
        Self {
            service: "tenon.keys".into(),
        }
    }

    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for KeychainStore {
    fn get(&self, name: &str) -> Option<String> {
        let out = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                &self.service,
                "-a",
                name,
                "-w",
            ])
            .output()
            .ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            None
        }
    }

    fn set(&self, name: &str, value: &str) {
        // 先删后写（幂等）
        let _ = std::process::Command::new("security")
            .args(["delete-generic-password", "-s", &self.service, "-a", name])
            .output();
        let _ = std::process::Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                &self.service,
                "-a",
                name,
                "-w",
                value,
                "-U",
            ])
            .output();
    }

    fn delete(&self, name: &str) {
        let _ = std::process::Command::new("security")
            .args(["delete-generic-password", "-s", &self.service, "-a", name])
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_keystore_roundtrip() {
        let ks = MemoryKeyStore::new();
        assert!(ks.get("GLM_KEY").is_none());
        ks.set("GLM_KEY", "abc");
        assert_eq!(ks.get("GLM_KEY").as_deref(), Some("abc"));
        ks.delete("GLM_KEY");
        assert!(ks.get("GLM_KEY").is_none());
    }

    #[test]
    fn env_keystore_reads_process_env() {
        std::env::set_var("TENON_TEST_ENV_KEY", "env-value");
        let ks = EnvKeyStore;
        assert_eq!(ks.get("TENON_TEST_ENV_KEY").as_deref(), Some("env-value"));
        assert!(ks.get("TENON_MISSING_KEY_XYZ").is_none());
    }
}
