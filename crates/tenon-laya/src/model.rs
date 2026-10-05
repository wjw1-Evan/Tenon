//! 模型格式与本地推理（daemon 内置 Rust 运行时，CPU，无额外进程，§9.8）。
//!
//! 模型 = JSON 线性分类器（词表 + 权重）：推理为词袋线性打分 + softmax /
//! sigmoid，纯 CPU、 Determinstic，单次 <200ms（典型 <1ms）。
//! 词法分析器：ASCII 词元 + 已注册中文词组命中（词表驱动）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::primitives::IntentLabel;

/// 模型文件根结构（`~/.tenon/models/laya/model.json`）。
/// v1.92 收敛：仅保留 intent / risk 头；模型文件中的 triage / prefilter
/// 头被 serde 静默忽略（旧文件兼容）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayaModel {
    pub name: String,
    pub version: u32,
    /// 意图预判（choice）
    #[serde(default)]
    pub intent: Option<ChoiceHead>,
    /// 命令风险（score，sigmoid）
    #[serde(default)]
    pub risk: Option<ScoreHead>,
}

/// 多分类头：词表 → 词索引；weights[词索引][标签] 线性权重 + bias。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceHead {
    pub labels: Vec<String>,
    pub vocab: HashMap<String, usize>,
    /// weights[vocab_index][label_index]
    pub weights: Vec<Vec<f32>>,
    pub bias: Vec<f32>,
}

/// 打分头：sigmoid(Σ w·x + b) → 0..1。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreHead {
    pub vocab: HashMap<String, usize>,
    pub weights: Vec<f32>,
    pub bias: f32,
}

impl LayaModel {
    /// 解析并校验模型结构（权重维度一致性）。
    pub fn parse(bytes: &[u8]) -> crate::Result<Self> {
        let model: LayaModel =
            serde_json::from_slice(bytes).map_err(|e| crate::LayaError::Load(e.to_string()))?;
        model.validate().map_err(crate::LayaError::Load)?;
        Ok(model)
    }

    fn validate(&self) -> std::result::Result<(), String> {
        if let Some(h) = &self.intent {
            h.validate("intent")?;
        }
        Ok(())
    }

    /// 意图预判（choice 原语）。
    pub fn classify_intent(&self, text: &str) -> Option<(IntentLabel, f32)> {
        let head = self.intent.as_ref()?;
        let (idx, prob) = head.classify(text)?;
        let label = IntentLabel::parse(head.labels.get(idx)?)?;
        Some((label, prob))
    }

    /// 命令风险（score 原语，0..1，未校准）。
    pub fn score_risk(&self, command: &str) -> Option<f32> {
        let head = self.risk.as_ref()?;
        let mut total = head.bias;
        let lower = command.to_lowercase();
        for token in tokenize(command) {
            if let Some(&i) = head.vocab.get(&token) {
                if let Some(&w) = head.weights.get(i) {
                    total += w;
                }
            }
        }
        // 中文风险词组命中
        for key in head.vocab.keys() {
            if !key.is_ascii() && lower.contains(key.as_str()) {
                if let Some(&i) = head.vocab.get(key) {
                    if let Some(&w) = head.weights.get(i) {
                        total += w;
                    }
                }
            }
        }
        Some(sigmoid(total))
    }
}

impl ChoiceHead {
    fn validate(&self, name: &str) -> std::result::Result<(), String> {
        if self.labels.is_empty() {
            return Err(format!("{name}: 标签为空"));
        }
        for row in &self.weights {
            if row.len() != self.labels.len() {
                return Err(format!("{name}: 权重行长度与标签数不一致"));
            }
        }
        if self.bias.len() != self.labels.len() {
            return Err(format!("{name}: bias 长度与标签数不一致"));
        }
        // 词表索引必须落在权重行范围内（允许多词共享同一索引）
        for idx in self.vocab.values() {
            if *idx >= self.weights.len() {
                return Err(format!("{name}: 词表索引 {idx} 越界"));
            }
        }
        Ok(())
    }

    fn classify(&self, text: &str) -> Option<(usize, f32)> {
        let mut scores = self.bias.clone();
        // ASCII 词元 + 中文词组命中（词表驱动的最大匹配）
        let mut hits: Vec<usize> = tokenize(text)
            .filter_map(|t| self.vocab.get(&t).copied())
            .collect();
        for key in zh_phrase_hits(text, &self.vocab) {
            if let Some(&i) = self.vocab.get(key) {
                hits.push(i);
            }
        }
        for i in hits {
            if let Some(row) = self.weights.get(i) {
                for (s, w) in scores.iter_mut().zip(row) {
                    *s += w;
                }
            }
        }
        let probs = softmax(&scores);
        let (best, prob) = probs
            .into_iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;
        Some((best, prob))
    }
}

/// 词法分析：ASCII 词元（小写）+ 词表中文词组命中（最大匹配）。
pub fn tokenize(text: &str) -> impl Iterator<Item = String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            current.push(ch);
        } else {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out.into_iter()
}

/// 中文词组命中（供模型侧 token 扩展：词表中的 zh 词条经 contains 命中）。
pub fn zh_phrase_hits<'a>(text: &str, vocab: &'a HashMap<String, usize>) -> Vec<&'a String> {
    vocab
        .keys()
        .filter(|k| !k.is_ascii() && text.contains(k.as_str()))
        .collect()
}

fn softmax(xs: &[f32]) -> Vec<f32> {
    let max = xs.iter().copied().fold(f32::MIN, f32::max);
    let exps: Vec<f32> = xs.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter()
        .map(|e| if sum > 0.0 { e / sum } else { 0.0 })
        .collect()
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_model() -> LayaModel {
        let mut vocab = HashMap::new();
        vocab.insert("fix".to_string(), 0);
        vocab.insert("修复".to_string(), 1);
        vocab.insert("explain".to_string(), 2);
        vocab.insert("解释".to_string(), 3);
        LayaModel {
            name: "test".into(),
            version: 1,
            intent: Some(ChoiceHead {
                labels: vec!["needs_change".into(), "pure_qa".into()],
                vocab: vocab.clone(),
                weights: vec![
                    vec![2.0, -2.0], // fix → needs_change
                    vec![2.0, -2.0], // 修复
                    vec![-2.0, 2.0], // explain → pure_qa
                    vec![-2.0, 2.0], // 解释
                ],
                bias: vec![0.0, 0.0],
            }),
            risk: Some(ScoreHead {
                vocab: {
                    let mut v = HashMap::new();
                    v.insert("rm".to_string(), 0);
                    v.insert("ls".to_string(), 1);
                    v
                },
                weights: vec![5.0, -5.0],
                bias: -1.0,
            }),
        }
    }

    #[test]
    fn intent_classification_zh_and_en() {
        let m = sample_model();
        let (label, conf) = m.classify_intent("请修复这个 bug").unwrap();
        assert_eq!(label, IntentLabel::NeedsChange);
        assert!(conf > 0.5);
        let (label2, _) = m.classify_intent("please explain the code").unwrap();
        assert_eq!(label2, IntentLabel::PureQa);
    }

    #[test]
    fn risk_score_sigmoid_range() {
        let m = sample_model();
        let risky = m.score_risk("rm -rf /tmp/x").unwrap();
        let safe = m.score_risk("ls -la").unwrap();
        assert!(risky > 0.5, "risky={risky}");
        assert!(safe < 0.5, "safe={safe}");
        assert!((0.0..=1.0).contains(&risky));
    }

    #[test]
    fn parse_rejects_inconsistent_weights() {
        let bad = r#"{"name":"x","version":1,"intent":{"labels":["a","b"],"vocab":{"w":0},"weights":[[1.0]],"bias":[0.0,0.0]}}"#;
        assert!(LayaModel::parse(bad.as_bytes()).is_err());
        let good = r#"{"name":"x","version":1,"intent":{"labels":["a","b"],"vocab":{"w":0},"weights":[[1.0,-1.0]],"bias":[0.0,0.0]}}"#;
        assert!(LayaModel::parse(good.as_bytes()).is_ok());
    }

    #[test]
    fn tokenizer_splits_ascii_words() {
        let toks: Vec<String> = tokenize("Fix the Login-flow!").collect();
        assert_eq!(toks, vec!["fix", "the", "login-flow"]);
    }
}
