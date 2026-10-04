//! §8.7 验收级性能预算 CI 门（服务层；WebView 帧率由 UI 手工矩阵兜底）。
//! 这些用例刻意 `ignore`：普通 `cargo test` 不受 CI 噪声影响，
//! performance job 通过 `--ignored` 显式运行。

use std::io::Write as _;
use std::time::{Duration, Instant};

use tenon_fs::{search, FileService, SearchOptions};

fn assert_under(name: &str, elapsed: Duration, budget: Duration) {
    assert!(elapsed < budget, "{name} 超出 {budget:?}：实际 {elapsed:?}");
}

#[test]
#[ignore = "performance budget；performance CI 显式运行"]
fn ten_mb_file_full_read_under_two_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let service = FileService::new(dir.path());
    let mut file = std::fs::File::create(dir.path().join("large.bin")).unwrap();
    // 10 MiB + 1 byte 越过旧只读阈值；v1.69 起必须全量读取（§8.7：打开并编辑 <2s）。
    let chunk = vec![b'a'; 1024 * 1024];
    for _ in 0..10 {
        file.write_all(&chunk).unwrap();
    }
    file.write_all(b"!").unwrap();
    file.sync_all().unwrap();

    let start = Instant::now();
    let view = service.read_file_view("large.bin").unwrap();
    assert_under(
        "10MB file full read",
        start.elapsed(),
        Duration::from_secs(2),
    );
    assert_eq!(view.content.len(), 10 * 1024 * 1024 + 1);
    assert_eq!(view.total_bytes, 10 * 1024 * 1024 + 1);
}

#[test]
#[ignore = "performance budget；performance CI 显式运行"]
fn global_search_first_result_on_100k_files_under_half_second() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("000")).unwrap();
    // 100 × 1,000 个 1 KiB 文件；目标标记存在于每个文件，验证搜索拿到
    // 第一个结果即可返回，而不是为首结果扫描全库。
    let body = "performance target tenon-budget\n".repeat(32);
    for group in 0..100 {
        let directory = root.join(format!("{group:03}"));
        if group > 0 {
            std::fs::create_dir(&directory).unwrap();
        }
        for index in 0..1000 {
            std::fs::write(directory.join(format!("f{index:04}.txt")), &body).unwrap();
        }
    }
    let options = SearchOptions {
        max_hits: 1,
        ..SearchOptions::default()
    };
    let start = Instant::now();
    let hits = search::search(root, "tenon-budget", &options).unwrap();
    assert_under(
        "100k-file search first result",
        start.elapsed(),
        Duration::from_millis(500),
    );
    assert_eq!(hits.len(), 1);
    assert!(hits[0].text.contains("tenon-budget"));
}
