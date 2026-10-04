//! 沙箱与写守卫（设计方案 §12.3）。
//!
//! M0 交付 = A/B 写守卫（本 crate 核心守卫）+ 网络三态建模 + Seatbelt
//! profile 生成（macOS 完整沙箱随 M1 启用，profile 先行可测）；命令执行
//! 统一走本 crate 的超时执行器（§9.2：单命令默认 120s）。

pub mod exec;
pub mod guard;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod network;
pub mod seatbelt;

pub use exec::{exec_argv, exec_command, ExecOutcome, SandboxSpec};
pub use guard::WriteGuard;
pub use network::NetworkState;
pub use seatbelt::seatbelt_profile;

/// 沙箱相关错误。
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("写路径越界: {0}（写操作限定在项目内，§12.3）")]
    PathEscape(String),
    #[error("拒绝写用户仓库 .git 内部（快照走独立 shadow 库，§10.3）: {0}")]
    GitInternal(String),
    #[error("符号链接逃逸: {0}")]
    SymlinkEscape(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}
