//! macOS Seatbelt profile 生成（设计方案 §12.3：M1 完整版；M0 先行生成可测）。
//!
//! 语义：
//! - 系统只读、写限项目内（+ 显式授予路径）；
//! - 网络随三态：断网全拒 / 镜像仅 registry / 域名仅白名单；
//! - 子进程执行允许常用工具链，deny 未显式允许的敏感操作。
//!
//! Linux 走 namespace + seccomp，随 M1 完整沙箱落地；Windows 经 WSL2 复用。

use crate::network::NetworkState;
use std::path::Path;

/// 生成 Seatbelt profile 文本。
pub fn seatbelt_profile(
    project_root: &Path,
    network: &NetworkState,
    extra_write_paths: &[std::path::PathBuf],
) -> String {
    let root = std::fs::canonicalize(project_root)
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mut p = String::new();
    p.push_str("(version 1)\n");
    p.push_str("(deny default)\n");
    // 进程基础能力：执行、读
    p.push_str("(allow process-exec)\n");
    p.push_str("(allow process-fork)\n");
    p.push_str("(allow file-read*)\n");
    // 系统运行所需（信号 / 系统只读元数据）
    p.push_str("(allow system-socket (socket-domain AF_UNIX))\n");
    p.push_str("(allow mach-lookup)\n");
    p.push_str("(allow sysctl-read)\n");

    // 写：项目内 + 显式授予路径（路径字面量须转义：`"` / `\` 会提前终止
    // 字符串字面量，产出无法解析的 profile，沙箱整体 fail-closed 拒执行）
    p.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        sb_escape(&root)
    ));
    for path in extra_write_paths {
        p.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            sb_escape(&path.to_string_lossy())
        ));
    }
    // 临时目录（构建 / 测试需要）
    p.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");
    p.push_str("(allow file-write* (subpath \"/tmp\"))\n");
    p.push_str("(allow file-write* (literal \"/dev/null\"))\n");

    // 网络三态（§12.3）
    match network {
        NetworkState::Offline => {
            p.push_str("(deny network*)\n");
        }
        NetworkState::MirrorProxy { proxy, .. } => {
            // v2.0 域过滤（design-v2.md §4.4）：镜像态收紧为「deny network* +
            // 仅放行回环代理端口」——沙箱内命令唯一出网通路是本地 registry
            // 白名单代理（域名解析与直连全部被拒，白名单由代理进程强制）；
            // 代理缺席时 fail-closed 同样全拒（不静默回退全网放行）。
            // 注：Seatbelt 网络地址格式为 "host:port" 且 host 仅支持 * / localhost
            //（实测 "127.0.0.1" 与裸 IP 均被 profile 解析器拒绝）。registries
            // 由代理进程消费。
            p.push_str("(deny network*)\n");
            if let Some(addr) = proxy {
                p.push_str(&format!(
                    "(allow network-outbound (remote ip \"localhost:{}\"))\n",
                    addr.port()
                ));
            }
        }
        NetworkState::DomainProxy { hosts: registries } => {
            p.push_str("(allow network*)\n");
            let _ = registries; // 流量经本地代理进程出网，代理进程侧再按白名单过滤
        }
    }
    p
}

/// Seatbelt profile 字符串字面量转义：`"` 终止字面量、`\` 改写转义序列，
/// 均会产出无法解析的 profile（沙箱 fail-closed 拒绝一切执行）。
fn sb_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn offline_profile_denies_network_and_confines_writes() {
        let root = Path::new("/tmp/proj");
        let p = seatbelt_profile(root, &NetworkState::offline(), &[]);
        assert!(p.contains("(deny default)"));
        assert!(p.contains("(deny network*)"));
        assert!(p.contains(&format!(
            "(allow file-write* (subpath \"{}\"))",
            root.display()
        )));
        assert!(p.contains("(allow file-read*)"), "系统只读");
    }

    #[test]
    fn mirror_proxy_allows_network_via_proxy_filtering() {
        // v2.0 域过滤（design-v2.md §4.4）：镜像态收紧——代理缺席 fail-closed
        // 全拒；代理在场仅放行回环（deny network* 之上叠加 loopback 例外）
        let p = seatbelt_profile(Path::new("/tmp/proj"), &NetworkState::mirror_default(), &[]);
        assert!(p.contains("(deny network*)"), "镜像态应拒绝默认网络");
        assert!(!p.contains("(allow network*)"), "不应再全网放行");
        assert!(
            !p.contains("(allow network-outbound"),
            "代理缺席不应有回环例外"
        );
        let with_proxy = NetworkState::MirrorProxy {
            registries: match NetworkState::mirror_default() {
                NetworkState::MirrorProxy { registries, .. } => registries,
                _ => unreachable!(),
            },
            proxy: Some("127.0.0.1:39876".parse().unwrap()),
        };
        let p2 = seatbelt_profile(Path::new("/tmp/proj"), &with_proxy, &[]);
        assert!(p2.contains("(deny network*)"));
        assert!(p2.contains("(allow network-outbound (remote ip \"localhost:39876\"))"));
    }

    #[test]
    fn extra_write_paths_included() {
        let p = seatbelt_profile(
            Path::new("/tmp/proj"),
            &NetworkState::offline(),
            &[PathBuf::from("/Users/x/.tenon/worktrees/w1")],
        );
        assert!(p.contains("/Users/x/.tenon/worktrees/w1"));
    }
}
