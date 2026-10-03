//! fuzzy 查找（设计方案 §7.4：Cmd/Ctrl+P 文件 / 符号 / 行）。

/// 单条 fuzzy 匹配结果。
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    pub text: String,
    pub score: i64,
}

const BASE: i64 = 10;
const CONSECUTIVE_BONUS: i64 = 12;
const WORD_START_BONUS: i64 = 18;
const STRING_START_BONUS: i64 = 25;
const GAP_PENALTY_CAP: i64 = 10;
const LENGTH_PENALTY_CAP: i64 = 4;

/// 子序列模糊匹配打分：
/// - 全部查询字符按序出现才命中（大小写不敏感）；
/// - 连续命中加分；词首（`/`、`_`、`-`、`.`、空格 后）与串首强加分；
/// - 命中间隔扣分（封顶）；候选越长略降权。
pub fn fuzzy_score(text: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().collect();
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut score = 0i64;
    let mut search_from = 0usize;
    let mut last_hit: Option<usize> = None;
    for qc in &q {
        let found = t
            .iter()
            .enumerate()
            .skip(search_from)
            .find(|(_, tc)| tc.to_lowercase().next() == Some(*qc))
            .map(|(i, _)| i)?;
        score += BASE;
        if found > 0 && last_hit == Some(found - 1) {
            score += CONSECUTIVE_BONUS;
        }
        if found == 0 {
            score += STRING_START_BONUS;
        } else if matches!(t[found - 1], '/' | '_' | '-' | '.' | ' ') {
            score += WORD_START_BONUS;
        }
        if let Some(l) = last_hit {
            let gap = found.saturating_sub(l + 1) as i64;
            score -= gap.min(GAP_PENALTY_CAP);
        }
        last_hit = Some(found);
        search_from = found + 1;
    }
    score -= (t.len() as i64 / 16).min(LENGTH_PENALTY_CAP);
    Some(score)
}

/// 对候选集做 fuzzy 排序（仅返回命中的）。
pub fn fuzzy_filter<'a, I: IntoIterator<Item = &'a str>>(
    candidates: I,
    query: &str,
) -> Vec<FuzzyMatch> {
    let mut out: Vec<FuzzyMatch> = candidates
        .into_iter()
        .filter_map(|c| {
            fuzzy_score(c, query).map(|s| FuzzyMatch {
                text: c.to_string(),
                score: s,
            })
        })
        .collect();
    out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.text.cmp(&b.text)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_prefix_beats_scattered() {
        let hits = fuzzy_filter(["src/main.rs", "sm/README", "misc.txt"], "src");
        assert_eq!(hits[0].text, "src/main.rs");
    }

    #[test]
    fn subsequence_matches() {
        let hits = fuzzy_filter(["src/main.rs", "docs/readme.md"], "smrs");
        assert_eq!(hits[0].text, "src/main.rs");
        assert!(!hits.iter().any(|h| h.text == "docs/readme.md"));
    }

    #[test]
    fn word_boundary_bonus() {
        // 串首（相当于词首）强于词中
        let a = fuzzy_score("circuit.rs", "circ").unwrap();
        let b = fuzzy_score("acircuit", "circ").unwrap();
        assert!(a > b, "词首命中应更优（{a} vs {b}）");
        // 分隔符后的词首强于词中
        let c = fuzzy_score("my-circuit", "circ").unwrap();
        let d = fuzzy_score("mycircuit", "circ").unwrap();
        assert!(c > d, "分隔符词首应更优（{c} vs {d}）");
    }

    #[test]
    fn empty_query_matches_all_with_zero() {
        let hits = fuzzy_filter(["a", "b"], "");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.score == 0));
    }

    #[test]
    fn no_match_returns_empty() {
        let hits = fuzzy_filter(["abc"], "xyz");
        assert!(hits.is_empty());
    }

    #[test]
    fn case_insensitive_multilingual() {
        assert!(fuzzy_score("ReadFile", "rf").is_some());
        assert!(fuzzy_score("解析器", "解").is_some());
    }

    #[test]
    fn scores_never_overflow() {
        // 极端长串 + 重复字符不 panic
        let long = "a".repeat(100_000);
        assert!(fuzzy_score(&long, "aaaa").is_some());
    }
}
