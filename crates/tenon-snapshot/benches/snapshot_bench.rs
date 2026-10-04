//! 性能基准（§18.4 / §8.7 预算进 CI）：snapshot 写入与恢复吞吐。
//! 无 criterion 依赖（std Instant 计时）；CI 挂载位见 .github/workflows/ci.yml。

use std::path::Path;
use std::time::Instant;

use tenon_snapshot::SnapshotStore;

fn setup(work: &Path, files: usize) -> SnapshotStore {
    std::fs::create_dir_all(work).unwrap();
    let snaps = work.join("snaps");
    for i in 0..files {
        std::fs::write(
            work.join(format!("f{i}.txt")),
            format!("content {i}\n{}", "x".repeat(200)),
        )
        .unwrap();
    }
    SnapshotStore::open(&snaps, "bench", work, 2).unwrap()
}

fn main() {
    let root = std::env::temp_dir().join(format!("tenon-bench-{}", std::process::id()));
    let _keep = root.clone();
    let store = setup(&root, 50);

    let t = Instant::now();
    let t1 = store.snapshot().unwrap();
    let snap_ms = t.elapsed().as_millis();

    std::fs::write(root.join("f0.txt"), "changed\n").unwrap();
    let t = Instant::now();
    let t2 = store.snapshot().unwrap();
    let incr_ms = t.elapsed().as_millis();

    let t = Instant::now();
    store.restore(&t1).unwrap();
    let restore_ms = t.elapsed().as_millis();

    println!(
        "snapshot: first {snap_ms}ms / incr {incr_ms}ms / restore {restore_ms}ms (50 files, trees {}..{})",
        &t1[..8],
        &t2[..8]
    );
    let _ = _keep;
}
