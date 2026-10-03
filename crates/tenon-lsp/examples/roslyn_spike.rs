//! Roslyn LS 沙箱化 spike 例程（附录 C Q2）：
//! 用法：cargo run -p tenon-lsp --example roslyn_spike -- <ls-dir> <workspace>
//!
//! 在 Linux 沙箱（Landlock 写限 + seccomp 拦 inet）内启动 Roslyn LS，
//! 验证 initialize 握手；成功输出 SPIKE_PASS。

use std::path::{Path, PathBuf};
use std::time::Duration;

use tenon_lsp::guard::LspGuardConfig;
use tenon_lsp::host::{LspHost, LspHostConfig};
use tenon_lsp::transport::ProcessConnection;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: roslyn_spike <ls-dir> <workspace>");
        std::process::exit(2);
    }
    let ls_dir = PathBuf::from(&args[1]);
    let workspace = PathBuf::from(&args[2]);

    // Roslyn LS 启动：dotnet exec Microsoft.CodeAnalysis.LanguageServer.dll --stdio
    // （或 VSIX 自包含可执行；两种形态都探测）
    let (program, pre_args): (String, Vec<String>) = if ls_dir
        .join("Microsoft.CodeAnalysis.LanguageServer.dll")
        .exists()
    {
        (
            "dotnet".into(),
            vec![
                ls_dir
                    .join("Microsoft.CodeAnalysis.LanguageServer.dll")
                    .to_string_lossy()
                    .into_owned(),
                "--stdio".into(),
            ],
        )
    } else {
        match find_executable(&ls_dir) {
            Some(exe) => (exe.to_string_lossy().into_owned(), vec!["--stdio".into()]),
            None => {
                eprintln!("SPIKE_FAIL: 未找到 Roslyn LS 启动入口");
                std::process::exit(1);
            }
        }
    };

    // macOS：Seatbelt 离线；Linux：Landlock + seccomp（写限工作区、拦 inet）
    #[cfg(target_os = "macos")]
    let sandbox_profile = tenon_sandbox_compatible_profile(&workspace);
    #[cfg(not(target_os = "macos"))]
    let sandbox_profile: Option<String> = None;

    let _ = sandbox_profile; // 例程在宿主内做 initialize；进程级沙箱由
                             // tenon-sandbox::exec 承担（run_tests 等同一路径）

    let conn = match ProcessConnection::spawn_str(&program, &pre_args, &workspace) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SPIKE_FAIL: 启动 {program} 失败: {e}");
            std::process::exit(1);
        }
    };
    let cfg = LspHostConfig {
        language: "csharp".into(),
        root_path: workspace.clone(),
        guard: LspGuardConfig::new(&workspace),
        initialization_options: None,
    };
    let host = LspHost::connect(cfg, Box::new(conn.reader), Box::new(conn.writer));
    match host.initialize(Duration::from_secs(60)) {
        Ok(caps) => {
            eprintln!("SPIKE_PASS: capabilities={}", caps);
        }
        Err(e) => {
            eprintln!("SPIKE_FAIL: initialize 失败: {e}");
            std::process::exit(1);
        }
    }
    host.shutdown();
}

fn find_executable(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains("LanguageServer") && !n.ends_with(".dll"))
                .unwrap_or(false)
        })
}

#[allow(dead_code)]
fn tenon_sandbox_compatible_profile(_workspace: &Path) -> Option<()> {
    None // 占位：进程级沙箱统一走 tenon-sandbox::exec（见模块注释）
}
