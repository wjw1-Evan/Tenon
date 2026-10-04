//! diff3 三方合并性能骨架（§10.3 回滚三方合并 / §8.6 共编合并共用内核）。
//! `cargo bench -p tenon-core`；骨架基线，M3 深化（冲突率分布）。

use std::time::Instant;

use tenon_core::merge::merge_three_way;

fn base_doc(sections: usize, lines_per: usize) -> String {
    (0..sections)
        .map(|s| {
            (0..lines_per)
                .map(move |l| format!("s{s}l{l}\n"))
                .collect::<String>()
        })
        .collect()
}

fn bench(name: &str, rounds: usize, build: impl Fn(usize) -> (String, String, String)) {
    let start = Instant::now();
    let mut conflicts = 0usize;
    for i in 0..rounds {
        let (base, ours, theirs) = build(i);
        match merge_three_way(&base, &ours, &theirs) {
            Ok(_) => {}
            Err(_) => conflicts += 1,
        }
    }
    let ms = start.elapsed().as_millis();
    eprintln!("{name}: {ms}ms（conflicts={conflicts}/{rounds}）");
}

fn main() {
    eprintln!("=== tenon-core merge3 bench（1k 行文档 × 100 轮）===");
    // 无冲突：两侧改不同段
    bench("disjoint-edits", 100, |i| {
        let base = base_doc(100, 10);
        let ours = base.replacen(&format!("s0l{i}\n"), &format!("ours-{i}\n"), 1);
        let theirs = base.replacen(&format!("s99l{}\n", (i + 1) % 10), "theirs\n", 1);
        (base, ours, theirs)
    });
    // 冲突：两侧改同一段
    bench("overlapping-edits", 100, |i| {
        let base = base_doc(100, 10);
        let ours = base.replacen(&format!("s50l{}\n", i % 10), "ours\n", 1);
        let theirs = base.replacen(&format!("s50l{}\n", i % 10), "theirs\n", 1);
        (base, ours, theirs)
    });
}
