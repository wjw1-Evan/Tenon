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

/// 操作系统钥匙串 / 凭据库（macOS Keychain、Linux libsecret、Windows PasswordVault）。
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
        #[cfg(target_os = "macos")]
        {
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
            if !out.status.success() {
                return None;
            }
            let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!value.is_empty()).then_some(value)
        }

        #[cfg(not(target_os = "macos"))]
        get_platform_secret(&self.service, name)
    }

    fn set(&self, name: &str, value: &str) {
        #[cfg(target_os = "macos")]
        {
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

        #[cfg(not(target_os = "macos"))]
        set_platform_secret(&self.service, name, value);
    }

    fn delete(&self, name: &str) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("security")
                .args(["delete-generic-password", "-s", &self.service, "-a", name])
                .output();
        }

        #[cfg(not(target_os = "macos"))]
        delete_platform_secret(&self.service, name);
    }
}

#[cfg(not(target_os = "macos"))]
fn get_platform_secret(service: &str, name: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("secret-tool")
            .args(["lookup", "service", service, "username", name])
            .output()
            .ok()?;
        if out.status.success() {
            let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!value.is_empty()).then_some(value)
        } else {
            None
        }
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"
$resource = $env:TENON_KEY_SERVICE
$account = $env:TENON_KEY_ACCOUNT
$vault = [Windows.Security.Credentials.PasswordVault,Windows.Security.Credentials,ContentType=WindowsRuntime]::new()
try {
  $credential = $vault.Retrieve($resource, $account)
  $credential.RetrievePassword()
  Write-Output $credential.Password
} catch {
  exit 1
}
"#;
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("TENON_KEY_SERVICE", service)
            .env("TENON_KEY_ACCOUNT", name)
            .output()
            .ok()?;
        if out.status.success() {
            let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!value.is_empty()).then_some(value)
        } else {
            None
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (service, name);
        None
    }
}

#[cfg(not(target_os = "macos"))]
fn set_platform_secret(service: &str, name: &str, value: &str) {
    #[cfg(target_os = "linux")]
    {
        let mut child = std::process::Command::new("secret-tool")
            .args([
                "store",
                "--label=Tenon model key",
                "service",
                service,
                "username",
                name,
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .ok();
        if let Some(mut child) = child.as_mut() {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(value.as_bytes());
            }
        }
        if let Some(mut child) = child.take() {
            let _ = child.wait();
        }
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"
$resource = $env:TENON_KEY_SERVICE
$account = $env:TENON_KEY_ACCOUNT
$password = $env:TENON_KEY_VALUE
$vault = [Windows.Security.Credentials.PasswordVault,Windows.Security.Credentials,ContentType=WindowsRuntime]::new()
try { $vault.Remove($vault.Retrieve($resource, $account)) | Out-Null } catch {}
$vault.Add([Windows.Security.Credentials.PasswordCredential]::new($resource, $account, $password))
"#;
        let _ = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("TENON_KEY_SERVICE", service)
            .env("TENON_KEY_ACCOUNT", name)
            .env("TENON_KEY_VALUE", value)
            .status();
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (service, name, value);
    }
}

#[cfg(not(target_os = "macos"))]
fn delete_platform_secret(service: &str, name: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("secret-tool")
            .args(["clear", "service", service, "username", name])
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"
$resource = $env:TENON_KEY_SERVICE
$account = $env:TENON_KEY_ACCOUNT
$vault = [Windows.Security.Credentials.PasswordVault,Windows.Security.Credentials,ContentType=WindowsRuntime]::new()
try { $vault.Remove($vault.Retrieve($resource, $account)) | Out-Null } catch {}
"#;
        let _ = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("TENON_KEY_SERVICE", service)
            .env("TENON_KEY_ACCOUNT", name)
            .status();
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (service, name);
    }
}

/// 解析链：环境变量为开发 / CI 快路径；缺失时读取操作系统凭据库。
pub struct ChainKeyStore {
    stores: Vec<Box<dyn KeyStore>>,
}

impl ChainKeyStore {
    pub fn new() -> Self {
        Self {
            stores: vec![Box::new(EnvKeyStore), Box::new(KeychainStore::new())],
        }
    }

    #[cfg(test)]
    fn with_stores(stores: Vec<Box<dyn KeyStore>>) -> Self {
        Self { stores }
    }
}

impl Default for ChainKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for ChainKeyStore {
    fn get(&self, name: &str) -> Option<String> {
        self.stores.iter().find_map(|store| store.get(name))
    }

    fn set(&self, name: &str, value: &str) {
        // 持久层放在链尾：不把模型密钥写入 daemon 进程环境。
        if let Some(store) = self.stores.last() {
            store.set(name, value);
        }
    }

    fn delete(&self, name: &str) {
        for store in &self.stores {
            store.delete(name);
        }
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

    #[test]
    fn chain_keystore_resolves_env_before_fallback_and_writes_persistent_layer() {
        struct FakeStore {
            values: std::sync::Mutex<BTreeMap<String, String>>,
        }
        impl KeyStore for FakeStore {
            fn get(&self, name: &str) -> Option<String> {
                self.values.lock().unwrap().get(name).cloned()
            }
            fn set(&self, name: &str, value: &str) {
                self.values
                    .lock()
                    .unwrap()
                    .insert(name.into(), value.into());
            }
            fn delete(&self, name: &str) {
                self.values.lock().unwrap().remove(name);
            }
        }
        let fallback_inner = std::sync::Arc::new(FakeStore {
            values: std::sync::Mutex::new(BTreeMap::from([(
                "CHAIN_KEY".into(),
                "keychain-value".into(),
            )])),
        });
        struct FallbackHandle(std::sync::Arc<FakeStore>);
        impl KeyStore for FallbackHandle {
            fn get(&self, name: &str) -> Option<String> {
                self.0.get(name)
            }
            fn set(&self, name: &str, value: &str) {
                self.0.set(name, value)
            }
            fn delete(&self, name: &str) {
                self.0.delete(name)
            }
        }
        let fallback = FallbackHandle(fallback_inner.clone());
        let chain = ChainKeyStore::with_stores(vec![Box::new(EnvKeyStore), Box::new(fallback)]);
        assert_eq!(chain.get("CHAIN_KEY").as_deref(), Some("keychain-value"));
        chain.set("CHAIN_KEY", "new-value");
        assert_eq!(chain.get("CHAIN_KEY").as_deref(), Some("new-value"));
        assert_eq!(
            fallback_inner
                .values
                .lock()
                .unwrap()
                .get("CHAIN_KEY")
                .map(String::as_str),
            Some("new-value")
        );
        chain.delete("CHAIN_KEY");
        assert!(chain.get("CHAIN_KEY").is_none());
    }

    #[test]
    fn chain_keystore_delete_propagates_to_all_layers() {
        let ks1 = MemoryKeyStore::new();
        ks1.set("KEY", "layer1");
        let ks2 = MemoryKeyStore::new();
        ks2.set("KEY", "layer2");
        let chain = ChainKeyStore::with_stores(vec![Box::new(ks1), Box::new(ks2)]);
        assert_eq!(chain.get("KEY").as_deref(), Some("layer1"));
        chain.delete("KEY");
        assert!(chain.get("KEY").is_none());
    }

    #[test]
    fn env_keystore_set_and_delete_do_not_panic() {
        let ks = EnvKeyStore;
        ks.set("TENON_NOOP", "v");
        ks.delete("TENON_NOOP");
    }
}
