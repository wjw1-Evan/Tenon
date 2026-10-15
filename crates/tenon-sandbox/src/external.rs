//! 统一外部进程沙箱总线（design-v2.md §4.3，v2.4）。
//!
//! 倒挂收口：v1 语言服务器与 MCP server 均为裸 spawn（design.md §12.1
//! 「语言服务器属 B 级沙箱进程」只是目标态；v1.145 留债「MCP 长驻 stdio
//! 进程暂不进 Seatbelt/Landlock 包装」）。本模块提供统一入口
//! [`spawn_external`]：声明式权限（写根 × 网络二态），macOS 经 sandbox-exec
//! profile（[`crate::seatbelt::external_profile`]）、Linux 经 pre_exec
//! Landlock+seccomp（复用 [`crate::linux::LinuxSandbox`]）、其他平台降级直跑
//! （写守卫兜底，§12.3 降级语义）。
//!
//! 沙箱内进程的 stdio 为管道（不受文件沙箱约束），LSP / MCP 的帧协议不受影响。
//! macOS profile 文件须存活到 sandbox-exec 读取（v1.150 同坑）——[`SpawnKeepalive`]
//! 由调用方随进程存活持有（Connection 结构字段），Drop 即清理。

use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// 外部进程网络策略（声明式；MCP 按服务器 `permissions` 是否含 `net:*`，
/// LSP 按语言包声明——v2.74 逐语言包收紧，tsserver / pyright offline、
/// rust-analyzer open，见 tenon-lsp `pack.rs`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalNetwork {
    Offline,
    Open,
}

/// 外部进程声明式权限。
#[derive(Debug, Clone)]
pub struct ExternalSpec {
    /// 可写根（首个为主根：项目 / 受管 worktree；系统临时目录与平台缓存
    /// 目录由本模块自动追加——node/npx 与语言服务器缓存所需）。
    pub write_roots: Vec<PathBuf>,
    pub network: ExternalNetwork,
}

impl ExternalSpec {
    /// 以工作根（项目 / worktree）构建常规外部进程权限。
    pub fn for_workspace(root: &std::path::Path, network: ExternalNetwork) -> Self {
        ExternalSpec {
            write_roots: vec![root.to_path_buf()],
            network,
        }
    }
}

/// spawn 产物：子进程 + profile 文件保活句柄（调用方随进程存活持有）。
pub struct SpawnedExternal {
    pub child: Child,
    pub keepalive: SpawnKeepalive,
}

/// profile 保活句柄（非 macOS / 降级路径为 Empty）。
#[derive(Default)]
pub struct SpawnKeepalive {
    #[allow(dead_code)]
    profile: Option<tempfile::TempPath>,
}

/// 补充写根：系统临时目录 + 平台包管理器 / 工具链缓存目录（node/npx 首次
/// 下载写 ~/.npm 与 ~/Library/Caches；如实列出——缓存目录属用户级可写面）。
fn effective_write_roots(spec: &ExternalSpec) -> Vec<PathBuf> {
    let mut roots = spec.write_roots.clone();
    roots.push(std::env::temp_dir());
    if let Some(home) = dirs_home() {
        if cfg!(target_os = "macos") {
            roots.push(home.join("Library/Caches"));
        }
        roots.push(home.join(".npm"));
        roots.push(home.join(".cache"));
    }
    roots
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// 包装并 spawn 长驻 stdio 外部进程（MCP server / 语言服务器，§4.3 总线）。
/// `envs` 为额外环境（调用方解析自 `env:VAR` 引用，密钥不落盘语义不变）。
pub fn spawn_external(
    program: &str,
    args: &[&str],
    cwd: &std::path::Path,
    spec: &ExternalSpec,
    envs: &[(&str, &str)],
) -> io::Result<SpawnedExternal> {
    let write_roots = effective_write_roots(spec);
    #[cfg(target_os = "macos")]
    {
        // 逃生舱（降级档）：TENON_SANDBOX_EXTERNAL=0 关闭外部进程沙箱——
        // 用户环境 sandbox-exec 异常时的开关；默认开。
        if sandbox_exec_available() && sandbox_external_enabled() {
            let profile_text = crate::seatbelt::external_profile(
                &write_roots,
                spec.network == ExternalNetwork::Open,
            );
            let profile = write_profile_file(&profile_text)?;
            let mut cmd = Command::new("/usr/bin/sandbox-exec");
            cmd.arg("-f").arg(profile.path());
            cmd.arg(program).args(args);
            cmd.current_dir(cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(external_stderr());
            for (k, v) in envs {
                cmd.env(k, v);
            }
            let child = cmd.spawn()?;
            return Ok(SpawnedExternal {
                child,
                keepalive: SpawnKeepalive {
                    profile: Some(profile.into_temp_path()),
                },
            });
        }
        // sandbox-exec 不可用 → 降级直跑（文档化降级，与 exec_command 同语义）
        let mut cmd = plain_command(program, args, cwd, envs);
        let child = cmd.spawn()?;
        // cfg 平台块结构里多分支互斥，块尾 return 不可省（后续 cfg 块并存）
        #[allow(clippy::needless_return)]
        Ok(SpawnedExternal {
            child,
            keepalive: SpawnKeepalive { profile: None },
        })
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let offline = spec.network == ExternalNetwork::Offline;
        let mut cmd = plain_command(program, args, cwd, envs);
        // v2.21：外部进程（沙箱总线）网络二态——offline 全拒 / open 放行，
        // 不进镜像态回环收紧（外部进程无代理通路语义）。
        let linux_sandbox = crate::linux::LinuxSandbox {
            offline,
            mirror_proxy_port: None,
            write_paths: write_roots,
        };
        unsafe {
            cmd.pre_exec(move || linux_sandbox.apply());
        }
        let child = cmd.spawn()?;
        Ok(SpawnedExternal {
            child,
            keepalive: SpawnKeepalive { profile: None },
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = write_roots;
        let mut cmd = plain_command(program, args, cwd, envs);
        let child = cmd.spawn()?;
        Ok(SpawnedExternal {
            child,
            keepalive: SpawnKeepalive { profile: None },
        })
    }
}

fn plain_command(
    program: &str,
    args: &[&str],
    cwd: &std::path::Path,
    envs: &[(&str, &str)],
) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(external_stderr());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd
}

/// 外部进程 stderr 去向：`TENON_EXTERNAL_STDERR=<路径>` 时落盘（排障通道，
/// 沙箱拒操作会在这里现形），否则 /dev/null（v1 行为）。
fn external_stderr() -> Stdio {
    match std::env::var_os("TENON_EXTERNAL_STDERR") {
        Some(path) => match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(f) => Stdio::from(f),
            Err(_) => Stdio::null(),
        },
        None => Stdio::null(),
    }
}

#[cfg(target_os = "macos")]
fn sandbox_exec_available() -> bool {
    std::path::Path::new("/usr/bin/sandbox-exec").exists()
}

/// 逃生舱开关：`TENON_SANDBOX_EXTERNAL=0` 关闭外部进程沙箱（默认开）。
pub fn sandbox_external_enabled() -> bool {
    std::env::var_os("TENON_SANDBOX_EXTERNAL")
        .map(|v| v != "0")
        .unwrap_or(true)
}

/// profile 落盘（NamedTempFile：随机名 + 0600，防预置/替换，v1.150 同款纪律）。
#[cfg(target_os = "macos")]
fn write_profile_file(profile: &str) -> io::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let dir = std::env::temp_dir().join("tenon-sbx");
    std::fs::create_dir_all(&dir)?;
    let mut f = tempfile::NamedTempFile::new_in(&dir)?;
    f.write_all(profile.as_bytes())?;
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[cfg(target_os = "macos")]
    #[test]
    fn external_offline_blocks_network_and_open_allows_writes_confined() {
        // ① offline 外部进程：网络被拒
        let dir = tempfile::tempdir().unwrap();
        let spec = ExternalSpec::for_workspace(dir.path(), ExternalNetwork::Offline);
        let mut spawned = spawn_external(
            "/bin/sh",
            &[
                "-c",
                "curl -sS --max-time 3 -o /dev/null https://example.com 2>&1; echo EXIT:$?",
            ],
            dir.path(),
            &spec,
            &[],
        )
        .unwrap();
        let mut out = String::new();
        let _ = spawned
            .child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out);
        let _ = spawned.child.wait();
        assert!(
            out.contains("EXIT:1") || out.contains("EXIT:6"),
            "offline 应拒网: {out}"
        );

        // ② open 外部进程：写根内成功、根外被拒（根外目标放 HOME 下——
        // 系统临时目录被自动追加为可写根（工具链需要），tempdir 互为兄弟
        // 目录不能当「根外」对照）
        let outside = dirs_home()
            .unwrap()
            .join(format!(".tenon-sbx-escape-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("escaped.txt");
        let script = format!(
            "echo ok > {d}/inside.txt; cat > {t} <<'X'\npwned\nX\necho W:$([ -f {t} ] && echo yes || echo no)",
            d = dir.path().to_string_lossy(),
            t = target.to_string_lossy()
        );
        let spec2 = ExternalSpec::for_workspace(dir.path(), ExternalNetwork::Open);
        let mut spawned2 =
            spawn_external("/bin/sh", &["-c", &script], dir.path(), &spec2, &[]).unwrap();
        let mut out2 = String::new();
        let _ = spawned2
            .child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out2);
        let _ = spawned2.child.wait();
        assert!(out2.contains("W:no"), "根外写应被拒: {out2}");
        assert!(dir.path().join("inside.txt").exists(), "根内写应成功");
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn stdio_piping_works_under_sandbox() {
        // 沙箱内进程 stdio 管道帧协议可用（MCP/LSP 的通信前提）
        let dir = tempfile::tempdir().unwrap();
        let spec = ExternalSpec::for_workspace(dir.path(), ExternalNetwork::Offline);
        let mut spawned =
            spawn_external("/bin/sh", &["-c", "echo frame-ok"], dir.path(), &spec, &[]).unwrap();
        let mut out = String::new();
        let _ = spawned
            .child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out);
        let _ = spawned.child.wait();
        assert!(out.contains("frame-ok"), "{out}");
    }
}
