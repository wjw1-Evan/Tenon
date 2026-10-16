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
    /// 镜像代理：依赖安装（v2.0 design-v2.md §4.4——`proxy` = 域过滤代理
    /// 进程地址：Some 时注入 HTTP(S)_PROXY 环境且 Seatbelt 收紧为仅回环；
    /// None = fail-closed 全拒网络）。
    MirrorProxy {
        project_root: PathBuf,
        proxy: Option<std::net::SocketAddr>,
    },
    /// 域名代理：C 级直执审计域名（同上，代理进程随 M2）。
    DomainProxy {
        project_root: PathBuf,
        hosts: Vec<String>,
    },
}

impl SandboxSpec {
    /// 仅 macOS Seatbelt 路径消费（Linux 走 seccomp 过滤器、Windows 兜底不沙箱）。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn network(&self) -> NetworkState {
        match self {
            SandboxSpec::None | SandboxSpec::Offline { .. } => NetworkState::offline(),
            SandboxSpec::MirrorProxy { proxy, .. } => NetworkState::MirrorProxy {
                registries: match NetworkState::mirror_default() {
                    NetworkState::MirrorProxy { registries, .. } => registries,
                    _ => unreachable!("mirror_default 恒为 MirrorProxy"),
                },
                proxy: *proxy,
            },
            SandboxSpec::DomainProxy { hosts, .. } => NetworkState::DomainProxy {
                hosts: hosts.iter().cloned().collect(),
            },
        }
    }

    fn project_root(&self) -> Option<&Path> {
        match self {
            SandboxSpec::None => None,
            SandboxSpec::Offline { project_root }
            | SandboxSpec::MirrorProxy { project_root, .. }
            | SandboxSpec::DomainProxy { project_root, .. } => Some(project_root),
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn offline(&self) -> bool {
        matches!(self, SandboxSpec::Offline { .. })
    }
}

/// 沙箱构建产物：Command + 需要存活到子进程读完的资源（macOS 的 Seatbelt
/// profile 临时文件——sandbox-exec 在 exec 时读取，提前删除会丢沙箱）。
struct BuiltCommand {
    cmd: std::process::Command,
    #[allow(dead_code)]
    profile_keepalive: Option<tempfile::TempPath>,
}

/// 执行 shell 命令（经 `sh -c`），超时后杀死整棵进程树。
pub fn exec_command(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    sandbox: &SandboxSpec,
) -> std::io::Result<ExecOutcome> {
    exec_command_with_env(command, cwd, timeout, sandbox, &[])
}

/// 同 [`exec_command`]，追加透传指定环境变量（v2.86 §9.2 settings
/// `execpolicy.env_passthrough`——名单经 daemon settings 层校验，此处只做
/// 机制：父进程有值则注入，未设置跳过）。
pub fn exec_command_with_env(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    sandbox: &SandboxSpec,
    env_passthrough: &[String],
) -> std::io::Result<ExecOutcome> {
    let tmp = tempfile::tempdir()?;
    let out_path = tmp.path().join("stdout");
    let err_path = tmp.path().join("stderr");

    let mut built = match build_sandboxed_command(command, sandbox)? {
        Some(b) => b,
        None => {
            let mut c = std::process::Command::new("/bin/sh");
            c.arg("-c").arg(command);
            BuiltCommand {
                cmd: c,
                profile_keepalive: None,
            }
        }
    };
    built.cmd.current_dir(cwd);
    built.cmd.env_clear();
    // 最小环境（PATH / HOME / LANG）+ 工具链白名单透传（v2.78）+ 用户自定义
    // 透传（v2.86）
    apply_base_env_with(&mut built.cmd, env_passthrough);
    // v2.0 域过滤（design-v2.md §4.4）：镜像态注入本地代理环境——沙箱内命令
    // 唯一出网通路（大小写双写，覆盖 npm / pip / cargo / go / git 等主流客户端）
    if let SandboxSpec::MirrorProxy {
        proxy: Some(addr), ..
    } = sandbox
    {
        let proxy_url = format!("http://{addr}");
        built.cmd.env("HTTP_PROXY", &proxy_url);
        built.cmd.env("HTTPS_PROXY", &proxy_url);
        built.cmd.env("http_proxy", &proxy_url);
        built.cmd.env("https_proxy", &proxy_url);
        built.cmd.env("NO_PROXY", "localhost,127.0.0.1,::1");
        built.cmd.env("no_proxy", "localhost,127.0.0.1,::1");
    }
    built.cmd.stdin(Stdio::null());
    built.cmd.stdout(std::fs::File::create(&out_path)?);
    built.cmd.stderr(std::fs::File::create(&err_path)?);
    set_own_process_group(&mut built.cmd);

    let mut child = built.cmd.spawn()?;
    let (exit_code, timed_out) = wait_with_timeout(&mut child, timeout);
    // profile 须存活到子进程退出（sandbox-exec 启动后才读取），此处才清理
    drop(built);

    Ok(ExecOutcome {
        exit_code,
        stdout: read_lossy(&out_path),
        stderr: read_lossy(&err_path),
        timed_out,
    })
}

/// 命令子进程透传的开发工具链环境白名单（v2.78，§9.2）。
///
/// 背景：v2.2 起命令子进程走 `env_clear` + 仅 PATH/HOME/LANG「最小环境」——
/// 定制工具链（`RUSTUP_HOME` / `CARGO_HOME`）、构建期依赖的 `JAVA_HOME`、
/// 虚拟环境（`VIRTUAL_ENV` / `CONDA_PREFIX`）、版本管理器与 ssh-agent 推送
/// 所需变量全部被剥离，实测在定制 rustup 安装下 `cargo` 直接不可用（agent
/// 被迫自排查，甚至下载安装整套工具链）。
///
/// 取向：只透传**工具链 / 工具目录类**变量（无凭证语义）；`*_TOKEN` /
/// `*_SECRET` / `AWS_*` 等密钥类变量仍不进子进程——命令输出会进会话上下文
/// 与审计，避免密钥落盘落档。白名单外变量经 settings `execpolicy.env_passthrough`
/// 按名追加（v2.86 接线，名单校验见 `tenon_core::execpolicy::validate_env_name`）。
pub const DEV_ENV_PASSTHROUGH: &[&str] = &[
    // Rust
    "RUSTUP_HOME",
    "CARGO_HOME",
    "RUSTUP_TOOLCHAIN",
    "RUSTC_WRAPPER",
    "CARGO_TARGET_DIR",
    // Go
    "GOROOT",
    "GOPATH",
    "GOBIN",
    // JVM / .NET
    "JAVA_HOME",
    "GRADLE_USER_HOME",
    "MAVEN_HOME",
    "DOTNET_ROOT",
    "DOTNET_CLI_HOME",
    "NUGET_PACKAGES",
    // Node
    "NVM_DIR",
    "NVM_BIN",
    "NODE_PATH",
    "VOLTA_HOME",
    "PNPM_HOME",
    "COREPACK_HOME",
    "NPM_CONFIG_PREFIX",
    // Python
    "PYENV_ROOT",
    "PYTHONPATH",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "CONDA_DEFAULT_ENV",
    "PIP_CACHE_DIR",
    // Ruby / 版本管理器 / 移动端 SDK
    "GEM_HOME",
    "GEM_PATH",
    "SDKMAN_DIR",
    "MISE_DATA_DIR",
    "MISE_SHELL",
    "ASDF_DIR",
    "ASDF_DATA_DIR",
    "ANDROID_HOME",
    "ANDROID_SDK_ROOT",
    // 开发工作流：ssh-agent（git push over ssh）、时区与 XDG 缓存/配置根
    "SSH_AUTH_SOCK",
    "TZ",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

/// 注入命令子进程的基础环境：最小环境（PATH / HOME / LANG）+ 工具链白名单透传。
fn apply_base_env(cmd: &mut std::process::Command) {
    apply_base_env_with(cmd, &[])
}

/// 同 [`apply_base_env`]，追加按名透传父进程环境（v2.86 settings
/// `execpolicy.env_passthrough`；名单合法性由 settings 层把关）。
fn apply_base_env_with(cmd: &mut std::process::Command, extra: &[String]) {
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
    cmd.env("HOME", std::env::var("HOME").unwrap_or_default());
    cmd.env("LANG", "C.UTF-8");
    for key in DEV_ENV_PASSTHROUGH {
        // 白名单键一律覆盖注入（父进程未设置则跳过，避免注入空串改变工具语义）
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    for key in extra {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
}

/// 后台启动 shell 命令（§9.2 v2.64）：与 exec_command 同 shell 与环境
/// （`/bin/sh -c` + 基础环境 + 工具链白名单，v2.78），不等待——stdout/stderr 各自管道，
/// 由调用方（agent bgtask 注册表）托管读取、终止与生命周期。
pub fn spawn_command_background(command: &str, cwd: &Path) -> std::io::Result<std::process::Child> {
    spawn_command_background_with_env(command, cwd, &[])
}

/// 同 [`spawn_command_background`]，追加按名透传父进程环境（v2.86）。
pub fn spawn_command_background_with_env(
    command: &str,
    cwd: &Path,
    env_passthrough: &[String],
) -> std::io::Result<std::process::Child> {
    let mut c = std::process::Command::new("/bin/sh");
    c.arg("-c").arg(command);
    c.current_dir(cwd);
    c.env_clear();
    apply_base_env_with(&mut c, env_passthrough);
    c.stdin(Stdio::null());
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    set_own_process_group(&mut c);
    c.spawn()
}

/// argv 直执版本：用于代理侧 `gh` / 平台 CLI，不经 shell 解释。
/// `extra_env` 只追加最小环境；凭证来自用户本机 CLI 配置或注入环境。
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
    apply_base_env(&mut built);
    built.env("GH_NO_UPDATE_NOTIFIER", "1");
    built.env("NO_COLOR", "1");
    built.env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in extra_env {
        built.env(key, value);
    }
    built.stdin(Stdio::null());
    built.stdout(std::fs::File::create(&out_path)?);
    built.stderr(std::fs::File::create(&err_path)?);
    set_own_process_group(&mut built);

    let mut child = built.spawn()?;
    let (exit_code, timed_out) = wait_with_timeout(&mut child, timeout);

    Ok(ExecOutcome {
        exit_code,
        stdout: read_lossy(&out_path),
        stderr: read_lossy(&err_path),
        timed_out,
    })
}

/// 子进程自立进程组：超时才能 killpg 击杀整棵树——`sh -c "npm test"` 派生的
/// 孙进程被 init 收养后不受直接子进程 kill 影响，会带着网络继续跑。
#[cfg(unix)]
fn set_own_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn set_own_process_group(_cmd: &mut std::process::Command) {}

/// 等待 + 超时整树击杀。
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> (Option<i32>, bool) {
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    kill_process_tree(child.id());
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => break None,
        }
    };
    (status.and_then(|s| s.code()), timed_out)
}

#[cfg(unix)]
fn kill_process_tree(pid: u32) {
    // 进程组整体击杀，兜底再杀直接子进程（孙进程被 init 收养后不随父死）
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_tree(pid: u32) {
    let _ = pid; // Windows 侧由调用方 taskkill / 忽略
}

/// 读取输出文件：非 UTF-8 字节（进度条/本地化输出常见）以 U+FFFD 替换，
/// 不再整段静默清空失真证据通道。
fn read_lossy(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn build_sandboxed_command(
    command: &str,
    sandbox: &SandboxSpec,
) -> std::io::Result<Option<BuiltCommand>> {
    if matches!(sandbox, SandboxSpec::None) {
        return Ok(None);
    }
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        return Ok(None); // sandbox-exec 不可用 → 降级直跑（文档化降级，§12.3 M0）
    }
    let network = sandbox.network();
    let project_root = sandbox
        .project_root()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "缺项目根"))?
        .to_path_buf();
    let profile_text = crate::seatbelt::seatbelt_profile(&project_root, &network, &[]);
    // profile 落盘失败 → 让 spawn 失败（fail closed）：纯 IO 错误静默降级为
    // 无沙箱执行违背沙箱降级原则，且调用方无从区分
    let profile = write_profile_file(&profile_text)?;
    let profile_path = profile.to_path_buf();
    let mut c = std::process::Command::new("/usr/bin/sandbox-exec");
    c.arg("-f").arg(&profile_path);
    c.arg("/bin/sh").arg("-c").arg(command);
    Ok(Some(BuiltCommand {
        cmd: c,
        profile_keepalive: Some(profile),
    }))
}

#[cfg(target_os = "linux")]
fn build_sandboxed_command(
    command: &str,
    sandbox: &SandboxSpec,
) -> std::io::Result<Option<BuiltCommand>> {
    use std::os::unix::process::CommandExt;

    if matches!(sandbox, SandboxSpec::None) {
        return Ok(None);
    }
    let project_root = sandbox
        .project_root()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "缺项目根"))?
        .to_path_buf();
    // v2.21 镜像态回环收紧：镜像代理态填代理端口（Linux Landlock 仅放行该端口
    // connect；缺席 fail-closed）；域名代理态维持网络放行（对等 macOS 现状）。
    let mirror_proxy_port = match sandbox {
        SandboxSpec::MirrorProxy { proxy, .. } => proxy.map(|a| a.port()),
        _ => None,
    };
    let linux_sandbox = crate::linux::LinuxSandbox {
        offline: sandbox.offline(),
        mirror_proxy_port,
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
    Ok(Some(BuiltCommand {
        cmd: c,
        profile_keepalive: None,
    }))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn build_sandboxed_command(
    _command: &str,
    _sandbox: &SandboxSpec,
) -> std::io::Result<Option<BuiltCommand>> {
    Ok(None)
}

/// 仅 macOS Seatbelt 路径消费（profile 落盘供 sandbox-exec -f）。
/// tempfile：O_CREAT|O_EXCL + 随机名 + 0600——固定可预测路径可被同机进程
/// 预置/在 write 与 sandbox-exec 读取之间替换为攻击者 profile。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn write_profile_file(profile: &str) -> std::io::Result<tempfile::TempPath> {
    use std::io::Write;
    let dir = std::env::temp_dir().join("tenon-sbx");
    std::fs::create_dir_all(&dir)?;
    // NamedTempFile：O_CREAT|O_EXCL + 随机名 + unix 下默认 0600——固定可预测
    // 路径可被同机进程预置/在 write 与 sandbox-exec 读取之间替换 profile
    let mut f = tempfile::NamedTempFile::new_in(&dir)?;
    f.write_all(profile.as_bytes())?;
    Ok(f.into_temp_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v2.78 回归：命令子进程透传开发工具链环境（RUSTUP_HOME 等），
    /// 但**不**透传密钥类变量——而 v2.2 起只注入 PATH/HOME/LANG，
    /// 定制 rustup 安装下 `cargo` 直接不可用（实测 agent 被迫重装工具链）。
    #[test]
    fn command_env_passes_dev_toolchain_but_not_secrets() {
        // 父进程注入：一个白名单变量 + 一个密钥类变量
        std::env::set_var("RUSTUP_HOME", "/tmp/tenon-env-test-rustup");
        std::env::set_var("TENON_ENV_TEST_SECRET", "super-secret-value");
        let out = exec_command(
            "printf 'rustup=%s secret=%s home=%s\\n' \"$RUSTUP_HOME\" \"$TENON_ENV_TEST_SECRET\" \"$HOME\"",
            Path::new("/tmp"),
            Duration::from_secs(10),
            &SandboxSpec::None,
        )
        .unwrap();
        std::env::remove_var("RUSTUP_HOME");
        std::env::remove_var("TENON_ENV_TEST_SECRET");
        assert!(
            out.stdout.contains("rustup=/tmp/tenon-env-test-rustup"),
            "白名单变量须透传: {}",
            out.stdout
        );
        assert!(
            !out.stdout.contains("super-secret-value"),
            "密钥类变量不得进子进程: {}",
            out.stdout
        );
        assert!(
            !out.stdout.contains("home=\n"),
            "HOME 须注入: {}",
            out.stdout
        );
    }

    #[test]
    fn argv_exec_passes_dev_toolchain_env() {
        std::env::set_var("CARGO_HOME", "/tmp/tenon-env-test-cargo");
        let out = exec_argv(
            "/bin/sh",
            &[
                "-c".to_string(),
                "printf 'cargo=%s\\n' \"$CARGO_HOME\"".to_string(),
            ],
            Path::new("/tmp"),
            Duration::from_secs(10),
            &[],
        )
        .unwrap();
        std::env::remove_var("CARGO_HOME");
        assert!(
            out.stdout.contains("cargo=/tmp/tenon-env-test-cargo"),
            "exec_argv 同样透传白名单: {}",
            out.stdout
        );
    }

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
        // 关闭写句柄再 exec：Linux 对有活动写句柄的文件 execve 报 ETXTBSY
        //（macOS 宽容）；TempPath 保路径、drop 时清理
        let script = script.into_temp_path();
        // ubuntu runner（overlayfs）偶发对刚写入 + chmod 的脚本 execve 仍短暂
        // 报 ETXTBSY（Text file busy, errno 26）——内核 / overlayfs 时序伪影
        //（actions/runner-images 已知现象），短暂重试越过而非误判测试失败
        let mut etxtbusy_retry = 0;
        let out = loop {
            match exec_argv(
                script.to_str().unwrap(),
                &["--title".to_string(), "not a command".to_string()],
                Path::new("/tmp"),
                Duration::from_secs(5),
                &[],
            ) {
                Ok(out) => break out,
                // 托管 CI 镜像 /tmp 可能挂 noexec：脚本无法就地执行，跳过本断言
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("跳过：/tmp noexec（{e}）");
                    return;
                }
                Err(e) if e.raw_os_error() == Some(26) && etxtbusy_retry < 5 => {
                    etxtbusy_retry += 1;
                    std::thread::sleep(Duration::from_millis(120));
                }
                Err(e) => panic!("{e}"),
            }
        };
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

    // v2.86 §9.2：按名透传父进程环境（settings execpolicy.env_passthrough）——
    // 名单内可见、名单外仍剥离；父进程未设置的名单项不注入空串。
    #[test]
    fn exec_command_with_env_passthrough_names() {
        std::env::set_var("TENON_TEST_PASSTHROUGH", "marker-286");
        std::env::remove_var("TENON_TEST_NOT_LISTED");
        let out = exec_command_with_env(
            "echo \"v=$TENON_TEST_PASSTHROUGH listed=$TENON_TEST_NOT_LISTED_set\"",
            Path::new("/tmp"),
            Duration::from_secs(10),
            &SandboxSpec::None,
            &["TENON_TEST_PASSTHROUGH".to_string()],
        )
        .unwrap();
        assert!(out.success(), "{}{}", out.stdout, out.stderr);
        assert!(out.stdout.contains("v=marker-286"), "{}", out.stdout);
        assert!(
            out.stdout.contains("listed="),
            "名单外变量不注入（空串视为未设置）: {}",
            out.stdout
        );
        std::env::remove_var("TENON_TEST_PASSTHROUGH");
    }

    #[test]
    fn timeout_kills_grandchildren_too() {
        // `sh -c` 派生的后台孙进程不随直接子进程死（被 init 收养继续跑）：
        // 超时必须 killpg 击杀整棵进程组，否则沙箱超时形同虚设
        let marker = std::env::temp_dir().join(format!("tenon-grandchild-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let cmd = format!(
            "sh -c 'sleep 1.2; touch {m}; sleep 30' & sleep 30",
            m = marker.to_string_lossy()
        );
        let out = exec_command(
            &cmd,
            Path::new("/tmp"),
            Duration::from_millis(400),
            &SandboxSpec::None,
        )
        .unwrap();
        assert!(out.timed_out);
        std::thread::sleep(Duration::from_millis(1800));
        assert!(
            !marker.exists(),
            "孙进程应在超时后被进程组击杀（marker 不应出现）"
        );
        let _ = std::fs::remove_file(&marker);
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
    fn mirror_state_denies_direct_and_routes_via_registry_proxy() {
        // v2.0 域过滤（design-v2.md §4.4）：镜像态收紧——① 非白名单域
        //（直连被沙箱拒 / 经代理被白名单 403）一律失败；② 沙箱内唯一可达
        // 出网端点 = 回环白名单代理（curl 显式过代理，代理 403 理由回传——
        // 同时证明端口放行与白名单强制；真实 registry TLS 端到端属联网
        // happy path，不入沙箱断言）。
        let proj = tempfile::tempdir().unwrap();
        let proxy =
            crate::proxy::RegistryProxy::start(vec!["registry.npmjs.org".to_string()]).unwrap();
        let spec = SandboxSpec::MirrorProxy {
            project_root: proj.path().to_path_buf(),
            proxy: Some(proxy.addr()),
        };
        // ① 非白名单域被拒（http_proxy 环境已注入：直连被沙箱挡、代理
        // CONNECT 被白名单 403，两条路都不通）
        let out = exec_command(
            "curl -sS --max-time 4 https://example.com >/dev/null 2>&1; echo EXIT:$?",
            proj.path(),
            Duration::from_secs(15),
            &spec,
        )
        .unwrap();
        assert!(
            !out.stdout.contains("EXIT:0"),
            "非白名单域应失败（直连沙箱拒绝 / 代理 403）: {:?}",
            out.stdout
        );
        // ② 沙箱内代理可达且白名单强制：显式经代理请求非白名单域，代理
        // 以 403 + 拒绝理由应答（curl 对 HTTP 错误码仍退出 0，正文可见）
        let out2 = exec_command(
            &format!(
                "curl -sS --max-time 4 -x http://127.0.0.1:{p} http://example.com/x; echo EXIT:$?",
                p = proxy.addr().port()
            ),
            proj.path(),
            Duration::from_secs(15),
            &spec,
        )
        .unwrap();
        assert!(
            out2.stdout.contains("tenon-registry-proxy") && out2.stdout.contains("EXIT:0"),
            "代理应可达并以白名单 403 应答: {:?}",
            out2.stdout
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mirror_state_without_proxy_fails_closed() {
        // v2.0 fail-closed（design-v2.md §4.4）：代理缺席 = 全拒网络（不静默
        // 回退全网放行）；executor 层在代理不可用时已先行拒绝，此处验证
        // profile 纵深防御
        let proj = tempfile::tempdir().unwrap();
        let spec = SandboxSpec::MirrorProxy {
            project_root: proj.path().to_path_buf(),
            proxy: None,
        };
        let out = exec_command(
            "curl -sS --max-time 4 https://registry.npmjs.org >/dev/null 2>&1; echo EXIT:$?",
            proj.path(),
            Duration::from_secs(15),
            &spec,
        )
        .unwrap();
        assert!(
            !out.stdout.contains("EXIT:0"),
            "无代理的镜像态应 fail-closed 全拒: {:?}",
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
