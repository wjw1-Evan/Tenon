//! 局域网配对（设计方案 §12.6 / M3）：显式开启 + 一次性配对码 +
//! 可吊销的设备令牌。默认关闭；开启后局域网设备凭配对码换取长效令牌，
//! 令牌可随时吊销（审计可查）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device: String,
    pub token: String,
    pub paired_at: String,
}

/// 配对管理器：显式开启才生成配对码；一次性、5 分钟有效。
#[derive(Default)]
pub struct PairingStore {
    enabled: Mutex<bool>,
    code: Mutex<Option<(String, Instant)>>,
    devices: Mutex<HashMap<String, PairedDevice>>, // device → 记录
}

const CODE_TTL: Duration = Duration::from_secs(300);

impl PairingStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 显式开启（默认关闭，§12.6）。开启即生成新配对码。
    pub fn enable(&self) -> String {
        *self.enabled.lock().expect("enabled lock") = true;
        self.new_code()
    }

    pub fn disable(&self) {
        *self.enabled.lock().expect("enabled lock") = false;
        *self.code.lock().expect("code lock") = None;
        self.devices.lock().expect("devices lock").clear();
    }

    pub fn is_enabled(&self) -> bool {
        *self.enabled.lock().expect("enabled lock")
    }

    fn new_code(&self) -> String {
        // 6 位数字配对码（人可输入）
        let code = format!("{:06}", crate::generate_token_hash() % 1_000_000);
        *self.code.lock().expect("code lock") = Some((code.clone(), Instant::now()));
        code
    }

    /// 重新生成配对码（旧码作废）。
    pub fn rotate_code(&self) -> String {
        self.new_code()
    }

    /// 配对：校验一次性码 → 签发设备令牌（吊销前长期有效）。
    pub fn pair(&self, device: &str, code: &str) -> Option<String> {
        if !self.is_enabled() {
            return None;
        }
        {
            let mut slot = self.code.lock().expect("code lock");
            match slot.as_ref() {
                Some((c, at)) if c == code && at.elapsed() < CODE_TTL => {
                    *slot = None; // 一次性
                }
                _ => return None,
            }
        }
        let token = crate::generate_token();
        let record = PairedDevice {
            device: device.to_string(),
            token: token.clone(),
            paired_at: chrono_now(),
        };
        self.devices
            .lock()
            .expect("devices lock")
            .insert(device.to_string(), record);
        Some(token)
    }

    /// 令牌校验（已配对且未吊销）。
    pub fn verify_token(&self, token: &str) -> bool {
        self.devices
            .lock()
            .expect("devices lock")
            .values()
            .any(|d| d.token == token)
    }

    /// 吊销设备令牌（可吊销，§12.6）。
    pub fn revoke(&self, device: &str) -> bool {
        self.devices
            .lock()
            .expect("devices lock")
            .remove(device)
            .is_some()
    }

    pub fn devices(&self) -> Vec<PairedDevice> {
        self.devices
            .lock()
            .expect("devices lock")
            .values()
            .cloned()
            .collect()
    }
}

fn chrono_now() -> String {
    // 轻量时间戳（避免引入 chrono 到此模块）
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}", d.as_secs()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default_pairing_fails() {
        let p = PairingStore::new();
        assert!(!p.is_enabled());
        assert!(p.pair("phone", "123456").is_none());
    }

    #[test]
    fn enable_pair_revoke_cycle() {
        let p = PairingStore::new();
        let code = p.enable();
        assert_eq!(code.len(), 6);
        let token = p.pair("phone", &code).expect("配对成功");
        assert!(p.verify_token(&token));
        // 配对码一次性
        assert!(p.pair("tablet", &code).is_none());
        // 吊销
        assert!(p.revoke("phone"));
        assert!(!p.verify_token(&token));
    }

    #[test]
    fn wrong_code_rejected_and_rotate_invalidates() {
        let p = PairingStore::new();
        let code = p.enable();
        assert!(p.pair("phone", "000000").is_none(), "错误码拒绝");
        let code2 = p.rotate_code();
        assert_ne!(code, code2);
        assert!(p.pair("phone", &code).is_none(), "旧码作废");
        assert!(p.pair("phone", &code2).is_some());
    }

    #[test]
    fn disable_revokes_all() {
        let p = PairingStore::new();
        let code = p.enable();
        let token = p.pair("phone", &code).unwrap();
        p.disable();
        assert!(!p.verify_token(&token), "关闭即全部吊销");
    }
}
