//! 沙箱执行性能骨架（§8.7 / §9.2：命令与验证不能被宿主层放大成交互卡顿）。
//! `cargo bench -p tenon-sandbox --bench exec_bench`。

use std::fs;
use std::time::{Duration, Instant};

use tenon_sandbox::exec::{exec_command, SandboxSpec};

fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("marker.txt"), "offline\n").unwrap();
    (dir, root)
}

fn bench(name: &str, rounds: usize, command: &str) {
    let (_guard, root) = workspace();
    let sandbox = SandboxSpec::Offline {
        project_root: root.clone(),
    };
    let start = Instant::now();
    let mut failures = 0usize;
    for _ in 0..rounds {
        let outcome = exec_command(command, &root, Duration::from_secs(5), &sandbox).unwrap();
        if !outcome.success() || outcome.stdout.trim() != "ok" {
            failures += 1;
        }
    }
    let per_round = start.elapsed().as_millis() / rounds.max(1) as u128;
    eprintln!("{name}: {per_round}ms/轮（failures={failures}/{rounds}）");
}

fn main() {
    eprintln!("=== tenon-sandbox exec bench（macOS Seatbelt / Linux namespace+Landlock）===");
    bench("offline-noop(20 rounds)", 20, "printf ok");
    bench(
        "offline-file-read(20 rounds)",
        20,
        "cat marker.txt >/dev/null && printf ok",
    );
}
