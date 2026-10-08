//! LSP 宿主（设计方案 §8.5 / §12.1 铁律七 / ADR-9）。
//!
//! - 一项目一 LSP 实例，编辑器与代理共用（多路复用：请求按 id 路由、
//!   通知按订阅分发）；
//! - **铁律七（ADR-9）**：宿主永不执行语言服务器下发的任意
//!   `workspace/executeCommand`（默认全拒，仅 `[lsp].allowed_commands`
//!   白名单放行）；`workspace/applyEdit` 与 `window/showDocument` 一律过
//!   写守卫与项目内路径检查；`client/registerCapability` 默认拒绝。

pub mod codec;
pub mod guard;
pub mod host;
pub mod manager;
pub mod pack;
pub mod transport;

pub use guard::{uri_within, GuardDecision, LspGuard, LspGuardConfig, ServerRequestReport};
pub use host::{LspHost, LspHostConfig, LspHostError, Notification};
pub use manager::{LspManager, LspManagerError};
pub use pack::{builtin_packs, pack_for_file};

use serde::{Deserialize, Serialize};

/// 简化的 JSON-RPC 消息（LSP 载荷）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonrpc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcMessage {
    pub fn request(id: i64, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: Some("2.0".into()),
            id: Some(id.into()),
            method: Some(method.into()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn notification(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: Some("2.0".into()),
            id: None,
            method: Some(method.into()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn response(id: i64, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: Some("2.0".into()),
            id: Some(id.into()),
            method: None,
            params: None,
            result: Some(result),
            error: None,
        }
    }

    pub fn error_response(id: i64, code: i64, message: &str) -> Self {
        Self {
            jsonrpc: Some("2.0".into()),
            id: Some(id.into()),
            method: None,
            params: None,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn is_response(&self) -> bool {
        // 有 id 且无 method 即为响应。不能以 result/error 出现与否判定：
        // result 为 null（hover/definition 无结果、shutdown 等的正常应答）
        // 经 serde 反序列化后与字段缺席不可区分，会整条被丢弃、
        // 调用方白等满超时。
        self.id.is_some() && self.method.is_none()
    }

    pub fn is_server_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }
}

/// JSON-RPC 错误码（LSP 规范常用值）。
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const REQUEST_DENIED: i64 = -32001;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_serialization_roundtrip() {
        let m = RpcMessage::request(1, "initialize", serde_json::json!({"rootUri": null}));
        let s = serde_json::to_string(&m).unwrap();
        let m2: RpcMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m2.method.as_deref(), Some("initialize"));
        assert_eq!(m2.id, Some(serde_json::json!(1)));
    }

    #[test]
    fn classification() {
        assert!(RpcMessage::response(1, serde_json::json!(null)).is_response());
        assert!(RpcMessage::request(2, "x", serde_json::json!({})).is_server_request());
        let n = RpcMessage::notification("publishDiagnostics", serde_json::json!({}));
        assert!(!n.is_response() && !n.is_server_request());
    }
}
