//! 性能基准（§18.4）：三方合并（万行文件）。

use std::time::Instant;

fn main() {
    let base: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
    let ours = base.replace("line 100\n", "OURS\n");
    let theirs = base.replace("line 9000\n", "THEIRS\n");

    let t = Instant::now();
    let merged = tenon_core::merge::merge_three_way(&base, &ours, &theirs).unwrap();
    let ms = t.elapsed().as_millis();

    assert!(merged.contains("OURS") && merged.contains("THEIRS"));
    println!("merge_three_way: 万行文件 {ms}ms");
}
