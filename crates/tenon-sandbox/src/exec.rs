//! 带超时的命令执行器（设计方案 §9.2：单命令默认 120s 超时；§12.3 沙箱三态）。
//!
//! 平台执行路径：
//! - **macOS**：`sandbox-exec` + Seatbelt profile（离线 deny network*；
//!   镜像/域名代理态放行网络，域过滤由代理进程承担，M2）；
//! - **Linux**：`pre_exec` 三层——network namespace（断网）、Landlock
//!   （写限项目内/临时目录）、seccomp BPF（拦 inet socket）；
//! - 其他 / 沙箱不可用：直接执行（写守卫兜底，§12.3 降级语义）。
//!
//! stdout/stderr 重定向到临时文件避免管道缓冲死锁。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::network::NetworkState;

#[derive(Debug, Clone)]
pub struct ExecOutcome {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl ExecOutcome {
    pub fn success(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

/// 沙箱规格（§12.3 三态；由工具语义决定，调用方传入）。
#[derive(Debug, Clone)]
pub enum SandboxSpec {
    /// 无沙箱（仅写守卫兜底；降级档 / 不可用平台）。
    None,
    /// 断网：测试 / 构建 / 纯分析。
    Offline { project_root: PathBuf },
    /// 镜像代理：依赖安装（域白名单过滤在代理进程，随 M2 落地；当前放行网络）。
    MirrorProxy { project_root: PathBuf },
    /// 域名代理：C 级直执审计域名（同上，代理进程随 M2）。
    DomainProxy {
        project_root: PathBuf,
        hosts: Vec<String>,
    },
}

impl SandboxSpec {
    fn network(&self) -> NetworkState {
        match self {
            SandboxSpec::None | SandboxSpec::Offline { .. } => NetworkState::offline(),
            SandboxSpec::MirrorProxy { .. } => NetworkState::mirror_default(),
            SandboxSpec::DomainProxy { hosts, .. } => NetworkState::DomainProxy {
                hosts: hosts.iter().cloned().collect(),
            },
        }
    }

    fn project_root(&self) -> Option<&Path> {
        match self {
            SandboxSpec::None => None,
            SandboxSpec::Offline { project_root }
            | SandboxSpec::MirrorProxy { project_root }
            | SandboxSpec::DomainProxy { project_root, .. } => Some(project_root),
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn offline(&self) -> bool {
        matches!(self, SandboxSpec::Offline { .. })
    }
}

/// 执行 shell 命令（经 `sh -c`），超时后杀死进程。
pub fn exec_command(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    sandbox: &SandboxSpec,
) -> std::io::Result<ExecOutcome> {
    let tmp = tempfile::tempdir()?;
    let out_path = tmp.path().join("stdout");
    let err_path = tmp.path().join("stderr");

    let mut built = build_sandboxed_command(command, sandbox).unwrap_or_else(|| {
        let mut c = std::process::Command::new("/bin/sh");
        c.arg("-c").arg(command);
        c
    });
    built.current_dir(cwd);
    built.env_clear();
    // 最小环境：命令工具链需要 PATH / HOME / LANG
    built.env("PATH", std::env::var("PATH").unwrap_or_default());
    built.env("HOME", std::env::var("HOME").unwrap_or_default());
    built.env("LANG", "C.UTF-8");
    built.stdin(Stdio::null());
    built.stdout(std::fs::File::create(&out_path)?);
    built.stderr(std::fs::File::create(&err_path)?);

    let mut child = built.spawn()?;
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(st) => break Some(st),
            None => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    let _ = child.kill();
                    break Some(child.wait()?);
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        }
    };

    let stdout = std::fs::read_to_string(&out_path).unwrap_or_default();
    let stderr = std::fs::read_to_string(&err_path).unwrap_or_default();

    Ok(ExecOutcome {
        exit_code: status.and_then(|s| s.code()),
        stdout,
        stderr,
        timed_out,
    })
}

/// argv 直执版本：用于代理侧 `gh` / 平台 CLI，不经 shell 解释。
/// `extra_env` 只追加最小环境；凭据仍来自用户本机 CLI 配置或注入环境。
pub fn exec_argv(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    extra_env: &[(&str, String)],
) -> std::io::Result<ExecOutcome> {
    let tmp = tempfile::tempdir()?;
    let out_path = tmp.path().join("stdout");
    let err_path = tmp.path().join("stderr");

    let mut built = std::process::Command::new(program);
    built.args(args).current_dir(cwd);
    built.env_clear();
    built.env("PATH", std::env::var("PATH").unwrap_or_default());
    built.env("HOME", std::env::var("HOME").unwrap_or_default());
    built.env("LANG", "C.UTF-8");
    built.env("GH_NO_UPDATE_NOTIFIER", "1");
    built.env("NO_COLOR", "1");
    built.env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in extra_env {
        built.env(key, value);
    }
    built.stdin(Stdio::null());
    built.stdout(std::fs::File::create(&out_path)?);
    built.stderr(std::fs::File::create(&err_path)?);

    let mut child = built.spawn()?;
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None if start.elapsed() >= timeout => {
                timed_out = true;
                let _ = child.kill();
                break Some(child.wait()?);
            }
            None => std::thread::sleep(Duration::from_millis(15)),
        }
    };

    Ok(ExecOutcome {
        exit_code: status.and_then(|s| s.code()),
        stdout: std::fs::read_to_string(&out_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&err_path).unwrap_or_default(),
        timed_out,
    })
}

#[cfg(target_os = "macos")]
fn build_sandboxed_command(command: &str, sandbox: &SandboxSpec) -> Option<std::process::Command> {
    if matches!(sandbox, SandboxSpec::None) {
        return None;
    }
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        return None; // sandbox-exec 不可用 → 降级直跑
    }
    let network = sandbox.network();
    let project_root = sandbox.project_root()?.to_path_buf();
    let profile_text = crate::seatbelt::seatbelt_profile(&project_root, &network, &[]);
    let pf = write_profile_file(&profile_text).ok()?;
    let mut c = std::process::Command::new("/usr/bin/sandbox-exec");
    c.arg("-f").arg(&pf);
    c.arg("/bin/sh").arg("-c").arg(command);
    Some(c)
}

#[cfg(target_os = "linux")]
fn build_sandboxed_command(command: &str, sandbox: &SandboxSpec) -> Option<std::process::Command> {
    use std::os::unix::process::CommandExt;

    if matches!(sandbox, SandboxSpec::None) {
        return None;
    }
    let project_root = sandbox.project_root()?.to_path_buf();
    let linux_sandbox = crate::linux::LinuxSandbox {
        offline: sandbox.offline(),
        write_paths: vec![
            project_root.clone(),
            std::env::temp_dir(),
            PathBuf::from("/dev/null"),
        ],
    };
    let mut c = std::process::Command::new("/bin/sh");
    c.arg("-c").arg(command);
    unsafe {
        c.pre_exec(move || linux_sandbox.apply());
    }
    Some(c)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn build_sandboxed_command(
    _command: &str,
    _sandbox: &SandboxSpec,
) -> Option<std::process::Command> {
    None
}

fn write_profile_file(profile: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("tenon-sbx");
    std::fs::create_dir_all(&dir)?;
    let name = format!(
        "profile-{}.sb",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = dir.join(name);
    std::fs::write(&path, profile)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_exit_code() {
        let out = exec_command(
            "echo hello; echo err >&2; exit 3",
            Path::new("/tmp"),
            Duration::from_secs(10),
            &SandboxSpec::None,
        )
        .unwrap();
        assert_eq!(out.exit_code, Some(3));
        assert!(out.stdout.contains("hello"));
        assert!(out.stderr.contains("err"));
        assert!(!out.timed_out);
        assert!(!out.success());
    }

    #[test]
    fn argv_exec_is_shell_free_and_redacts_nothing() {
        let script = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            script.path(),
            "#!/bin/sh\nprintf 'argv=%s\\n' \"$*\"\nprintf 'shell=%s\\n' \"$0\"\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(script.path(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let out = exec_argv(
            script.path().to_str().unwrap(),
            &["--title".to_string(), "not a command".to_string()],
            Path::new("/tmp"),
            Duration::from_secs(5),
            &[],
        )
        .unwrap();
        assert!(out.success(), "{}{}", out.stdout, out.stderr);
        assert!(out.stdout.contains("argv=--title not a command"));
    }

    #[test]
    fn timeout_kills_command() {
        let start = Instant::now();
        let out = exec_command(
            "sleep 30",
            Path::new("/tmp"),
            Duration::from_millis(300),
            &SandboxSpec::None,
        )
        .unwrap();
        assert!(out.timed_out);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn large_output_does_not_deadlock() {
        let out = exec_command(
            "seq 1 200000",
            Path::new("/tmp"),
            Duration::from_secs(30),
            &SandboxSpec::None,
        )
        .unwrap();
        assert!(out.success());
        assert!(out.stdout.contains("200000"));
    }

    #[test]
    fn runs_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let out = exec_command(
            "pwd",
            dir.path(),
            Duration::from_secs(10),
            &SandboxSpec::None,
        )
        .unwrap();
        assert!(out.success());
        let expected =
            std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
        let got = std::path::PathBuf::from(out.stdout.trim_end());
        assert_eq!(got, expected, "pwd={}", out.stdout.trim_end());
    }

    // ---------- §12.3 网络三态断言 + §18.2 逃逸套件（macOS 真实 Seatbelt） ----------

    #[cfg(target_os = "macos")]
    fn offline_spec(root: &Path) -> SandboxSpec {
        SandboxSpec::Offline {
            project_root: root.to_path_buf(),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_suite_offline_blocks_network() {
        // §18.2 断网态网络调用：curl 被沙箱拒绝
        let proj = tempfile::tempdir().unwrap();
        let out = exec_command(
            "curl -sS --max-time 3 https://example.com 2>&1; echo EXIT:$?",
            proj.path(),
            Duration::from_secs(15),
            &offline_spec(proj.path()),
        )
        .unwrap();
        assert!(
            out.stdout.contains("EXIT:1")
                || out.stdout.contains("EXIT:6")
                || out.stdout.contains("Operation not permitted")
                || out.stdout.contains("Could not resolve host"),
            "断网态网络调用应被拒绝: {:?}",
            out.stdout
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_suite_write_outside_project_blocked() {
        // §18.2 路径逃逸：hooks / 脚本写项目外被拒
        let proj = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("escaped.txt");
        let cmd = format!(
            "cat > {t} <<'X'\npwned\nX\necho WROTE:$([ -f {t} ] && echo yes || echo no)",
            t = target.to_string_lossy()
        );
        let out = exec_command(
            &cmd,
            proj.path(),
            Duration::from_secs(10),
            &offline_spec(proj.path()),
        )
        .unwrap();
        assert!(
            out.stdout.contains("WROTE:no"),
            "项目外写应被沙箱拒绝: {:?} {:?}",
            out.stdout,
            out.stderr
        );
        assert!(!target.exists());
        // 项目内写不受影响
        let inside = proj.path().join("inside.txt");
        let cmd2 = format!("echo ok > {}", inside.to_string_lossy());
        let out2 = exec_command(
            &cmd2,
            proj.path(),
            Duration::from_secs(10),
            &offline_spec(proj.path()),
        )
        .unwrap();
        assert!(out2.success() && inside.exists(), "项目内写应成功");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_suite_hook_subprocesses_contained() {
        // §18.2 hooks 触发（npm postinstall / pre-commit 语义）：
        // 父脚本派生子进程再写项目外 + 联网——整棵进程树都被 Seatbelt 覆盖
        let proj = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("hook-escape.txt");
        let cmd = format!(
            "sh -c 'sh -c \"echo pwned > {t}; curl -sS --max-time 2 https://example.com >/dev/null 2>&1; echo HOOK:$([ -f {t} ] && echo wrote || echo blocked)\"'",
            t = target.to_string_lossy()
        );
        let out = exec_command(
            &cmd,
            proj.path(),
            Duration::from_secs(15),
            &offline_spec(proj.path()),
        )
        .unwrap();
        assert!(
            out.stdout.contains("HOOK:blocked"),
            "hooks 子进程逃逸应被拦截: {:?}",
            out.stdout
        );
        assert!(!target.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mirror_proxy_allows_network() {
        // 镜像代理态：网络可达（域过滤在代理进程，M2；此处断言不误伤）
        let proj = tempfile::tempdir().unwrap();
        let spec = SandboxSpec::MirrorProxy {
            project_root: proj.path().to_path_buf(),
        };
        let out = exec_command(
            "curl -sS --max-time 5 -o /dev/null -w '%{{http_code}}' https://example.com; echo",
            proj.path(),
            Duration::from_secs(20),
            &spec,
        )
        .unwrap();
        assert!(
            !out.stdout.contains("Operation not permitted"),
            "镜像代理态不应误断网络: {:?}",
            out.stdout
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_offline_blocks_network_via_seccomp() {
        let proj = tempfile::tempdir().unwrap();
        let spec = SandboxSpec::Offline {
            project_root: proj.path().to_path_buf(),
        };
        let out = exec_command(
            "curl -sS --max-time 3 https://example.com 2>&1; echo EXIT:$?",
            proj.path(),
            Duration::from_secs(15),
            &spec,
        )
        .unwrap();
        // seccomp 返回 ENETDOWN → curl 网络失败
        assert!(
            out.stdout.contains("Network is down")
                || out.stdout.contains("EXIT:4")
                || out.stdout.contains("EXIT:6"),
            "断网态网络调用应失败: {:?}",
            out.stdout
        );
    }
}
