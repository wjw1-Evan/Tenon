//! snapshot 性能骨架（§10.3 快照时机不变式：每个 B 级写前一次）。
//! `cargo bench -p tenon-snapshot`；骨架基线，M3 深化（alternates 大仓）。

use std::fs;
use std::time::Instant;

use tenon_snapshot::SnapshotStore;

fn seed_tree(root: &std::path::Path, files: usize, lines: usize) {
    fs::create_dir_all(root.join("src")).unwrap();
    for i in 0..files {
        let body = format!(
            "fn f{i}() {{\n{}\n}}\n",
            (0..lines).map(|j| format!("    let _x{j} = {j};")).collect::<String>()
        );
        fs::write(root.join("src").join(format!("m{i}.rs")), body).unwrap();
    }
}

/// 返回 (增量快照 ms/轮, restore ms)。
fn bench_snapshot(files: usize, lines: usize, rounds: usize) -> (u128, u128) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    let project_id = format!("bench-{files}x{lines}");
    let store = SnapshotStore::open(
        &dir.path().join("snaps"),
        &project_id,
        &project,
        2,
    )
    .unwrap();
    seed_tree(&project, files, lines);
    // 预热一次（建对象库）；留存首个快照供 restore 计量
    let first_tree = store.snapshot().unwrap();
    let start = Instant::now();
    for r in 0..rounds {
        // 每轮改一个文件再快照（增量场景）
        fs::write(
            project.join("src").join(format!("m{}.rs", r % files)),
            format!("// touched {r}\n"),
        )
        .unwrap();
        store.snapshot().unwrap();
    }
    let incr_ms = start.elapsed().as_millis() / rounds as u128;
    // restore 吞吐（§8.7 预算进 CI 的挂载位）
    let t = Instant::now();
    store.restore(&first_tree).unwrap();
    let restore_ms = t.elapsed().as_millis();
    (incr_ms, restore_ms)
}

fn main() {
    let cases = [
        ("small(20 files×50 lines, 5 rounds)", 20, 50, 5),
        ("medium(100 files×100 lines, 5 rounds)", 100, 100, 5),
        ("large(400 files×200 lines, 3 rounds)", 400, 200, 3),
    ];
    eprintln!("=== tenon-snapshot bench（增量快照 ms/轮 + restore ms）===");
    for (name, files, lines, rounds) in cases {
        let (incr, restore) = bench_snapshot(files, lines, rounds);
        eprintln!("{name}: incr {incr}ms/轮, restore {restore}ms");
    }
}
