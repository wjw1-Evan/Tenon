//! 三方合并（设计方案 §8.6 人机共编）：base / ours（代理）/ theirs（用户脏缓冲）
//! 行级 diff3 合并；不相交改动自动合并，同区改动 → 冲突。

use similar::{DiffTag, TextDiff};

/// 合并冲突（供三栏预览）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    /// ours（代理）改动后的完整内容
    pub ours: String,
    /// theirs（用户脏缓冲）完整内容
    pub theirs: String,
    /// base（双方共同祖先）完整内容
    pub base: String,
}

/// 行级三方合并：
/// - 一侧改动 / 两侧不相交改动 → 合并结果；
/// - 两侧相同改动 → 去重取一；
/// - 同区不同改动 → `Err(MergeConflict)`（UI 三栏呈现，§8.6）。
pub fn merge_three_way(base: &str, ours: &str, theirs: &str) -> Result<String, MergeConflict> {
    let ours_hunks = hunks(base, ours);
    let theirs_hunks = hunks(base, theirs);

    let mut result = String::new();
    let mut base_pos = 0usize; // 0-based，指向 base 已消费行数
    let mut oi = 0usize;
    let mut ti = 0usize;

    let base_lines: Vec<&str> = split_lines(base);

    loop {
        let next_ours = ours_hunks.get(oi);
        let next_theirs = theirs_hunks.get(ti);
        let (hunk, side) = match (next_ours, next_theirs) {
            (None, None) => break,
            (Some(o), None) => (o.clone(), Side::Ours),
            (None, Some(t)) => (t.clone(), Side::Theirs),
            (Some(o), Some(t)) => {
                if o.base_start <= t.base_start {
                    (o.clone(), Side::Ours)
                } else {
                    (t.clone(), Side::Theirs)
                }
            }
        };

        // 与另一侧的 hunk 是否重叠（区间相交，或同位置的插入对插入）
        let overlap = match side {
            Side::Ours => theirs_hunks
                .get(ti)
                .map(|t| overlaps(&hunk, t))
                .unwrap_or(false),
            Side::Theirs => ours_hunks
                .get(oi)
                .map(|o| overlaps(o, &hunk))
                .unwrap_or(false),
        };

        if overlap {
            let o = ours_hunks[oi].clone();
            let t = theirs_hunks[ti].clone();
            if o.replacement == t.replacement && o.base_start == t.base_start {
                // 两侧相同改动：取一次
                emit(&mut result, &base_lines, base_pos, o.base_start);
                result.push_str(&o.replacement);
                base_pos = o.base_end;
                oi += 1;
                ti += 1;
                continue;
            }
            return Err(MergeConflict {
                ours: ours.to_string(),
                theirs: theirs.to_string(),
                base: base.to_string(),
            });
        }

        // 无重叠：先补 base 中该 hunk 之前的行
        emit(&mut result, &base_lines, base_pos, hunk.base_start);
        result.push_str(&hunk.replacement);
        base_pos = hunk.base_end;
        match side {
            Side::Ours => oi += 1,
            Side::Theirs => ti += 1,
        }
    }

    // 尾部
    emit(&mut result, &base_lines, base_pos, base_lines.len());
    Ok(result)
}

#[derive(Clone, Copy)]
enum Side {
    Ours,
    Theirs,
}

/// 一个改动块：base 区间 [base_start, base_end) → replacement 文本。
#[derive(Clone)]
struct Hunk {
    base_start: usize,
    base_end: usize,
    replacement: String,
}

fn overlaps(a: &Hunk, b: &Hunk) -> bool {
    a.base_start < b.base_end && b.base_start < a.base_end
        || (a.base_start == b.base_start && a.base_end == b.base_end)
}

fn split_lines(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

fn emit(out: &mut String, base_lines: &[&str], from: usize, to: usize) {
    for line in base_lines.iter().take(to).skip(from) {
        out.push_str(line);
    }
}

fn hunks(base: &str, other: &str) -> Vec<Hunk> {
    let other_lines: Vec<&str> = split_lines(other);
    let diff = TextDiff::from_lines(base, other);
    let mut out = Vec::new();
    for op in diff.ops() {
        match op.tag() {
            DiffTag::Equal => {}
            _ => {
                let new_range = op.new_range();
                let replacement: String = other_lines[new_range.start..new_range.end].concat();
                out.push(Hunk {
                    base_start: op.old_range().start,
                    base_end: op.old_range().end,
                    replacement,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";

    #[test]
    fn one_side_change_applies() {
        let ours = BASE.replace("l5", "OURS");
        let merged = merge_three_way(BASE, &ours, BASE).unwrap();
        assert!(merged.contains("OURS"));
    }

    #[test]
    fn disjoint_changes_merge_cleanly() {
        // ours 改 l2，theirs 改 l8
        let ours = BASE.replace("l2", "OURS2");
        let theirs = BASE.replace("l8", "THEIRS8");
        let merged = merge_three_way(BASE, &ours, &theirs).unwrap();
        assert!(merged.contains("OURS2"));
        assert!(merged.contains("THEIRS8"));
        assert!(merged.contains("l5"));
    }

    #[test]
    fn identical_changes_deduplicate() {
        let ours = BASE.replace("l3", "BOTH");
        let theirs = BASE.replace("l3", "BOTH");
        let merged = merge_three_way(BASE, &ours, &theirs).unwrap();
        assert_eq!(merged.matches("BOTH").count(), 1);
    }

    #[test]
    fn same_region_conflicts() {
        // 两侧改同一行不同内容 → 冲突
        let ours = BASE.replace("l5", "OURS5");
        let theirs = BASE.replace("l5", "THEIRS5");
        let err = merge_three_way(BASE, &ours, &theirs).unwrap_err();
        assert_eq!(err.ours, ours);
        assert_eq!(err.theirs, theirs);
    }

    #[test]
    fn adjacent_lines_no_conflict() {
        // 相邻行（不重叠区间）各自改动 → 干净合并
        let ours = BASE.replace("l4", "OURS4");
        let theirs = BASE.replace("l5", "THEIRS5");
        let merged = merge_three_way(BASE, &ours, &theirs).unwrap();
        assert!(merged.contains("OURS4") && merged.contains("THEIRS5"));
    }

    #[test]
    fn appends_both_at_tail() {
        let base = "a\n";
        let ours = "a\nours\n";
        let theirs = "a\ntheirs\n";
        // 尾部同位置插入不同内容 → 冲突（同区）
        assert!(merge_three_way(base, ours, theirs).is_err());
        // 相同追加 → 去重
        let merged = merge_three_way(base, ours, "a\nours\n").unwrap();
        assert_eq!(merged, ours);
    }

    #[test]
    fn deletions_merge() {
        let ours = BASE.replace("l2\n", "");
        let theirs = BASE.replace("l8\n", "");
        let merged = merge_three_way(BASE, &ours, &theirs).unwrap();
        assert!(!merged.contains("l2"));
        assert!(!merged.contains("l8"));
        assert!(merged.contains("l1") && merged.contains("l10"));
    }
}
