//! 密钥检测与脱敏（设计方案 §12.4）。
//!
//! `.env` 与常见 Key 模式检测 → 默认拦截不进上下文；拦截同样覆盖
//! `git_read` 输出（历史中曾提交的密钥同样脱敏）；Trace 标注拦截 / 放行。

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    AwsAccessKey,
    OpenAi,
    Anthropic,
    GitHub,
    Slack,
    Google,
    BigModel,
    PrivateKey,
    GenericAssign,
}

impl SecretKind {
    pub fn label(self) -> &'static str {
        match self {
            SecretKind::AwsAccessKey => "aws_access_key",
            SecretKind::OpenAi => "openai_key",
            SecretKind::Anthropic => "anthropic_key",
            SecretKind::GitHub => "github_token",
            SecretKind::Slack => "slack_token",
            SecretKind::Google => "google_key",
            SecretKind::BigModel => "bigmodel_key",
            SecretKind::PrivateKey => "private_key",
            SecretKind::GenericAssign => "generic_secret",
        }
    }
}

struct Pattern {
    kind: SecretKind,
    re: Regex,
}

fn patterns() -> &'static [Pattern] {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Pattern {
                kind: SecretKind::AwsAccessKey,
                re: Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap(),
            },
            Pattern {
                kind: SecretKind::OpenAi,
                re: Regex::new(r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b").unwrap(),
            },
            Pattern {
                kind: SecretKind::Anthropic,
                re: Regex::new(r"\bsk-ant-[A-Za-z0-9_-]{20,}\b").unwrap(),
            },
            // Stripe 风格：sk_live_ / sk_test_ / rk_live_
            Pattern {
                kind: SecretKind::GenericAssign,
                re: Regex::new(r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{10,}\b").unwrap(),
            },
            Pattern {
                kind: SecretKind::GitHub,
                re: Regex::new(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{20,}\b")
                    .unwrap(),
            },
            Pattern {
                kind: SecretKind::Slack,
                re: Regex::new(r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b").unwrap(),
            },
            Pattern {
                kind: SecretKind::Google,
                re: Regex::new(r"\bAIza[0-9A-Za-z_-]{35}\b").unwrap(),
            },
            // 智谱 / GLM 风格 key：32 位十六进制 . 16 位字母数字
            Pattern {
                kind: SecretKind::BigModel,
                re: Regex::new(r"\b[0-9a-f]{32}\.[A-Za-z0-9]{16}\b").unwrap(),
            },
            Pattern {
                kind: SecretKind::PrivateKey,
                re: Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
            },
            Pattern {
                kind: SecretKind::GenericAssign,
                re: Regex::new(
                    // 允许 diff 行首 +/- 前缀（§12.4：拦截覆盖 git_read 输出）
                    r#"(?im)^[+\-\s]*(?:export\s+)?([A-Z][A-Z0-9_]{2,}?(?:_KEY|_SECRET|_TOKEN|_PASSWORD)|API_?KEY|SECRET|PASSWORD)\s*[=:]\s*["']?([^\s"']{8,})["']?\s*$"#
                )
                .unwrap(),
            },
        ]
    })
}

/// 扫描文本中出现的密钥种类（用于 Trace 标注拦截 / 放行，§12.4）。
pub fn scan(text: &str) -> Vec<SecretKind> {
    let mut found = Vec::new();
    for p in patterns() {
        if p.re.is_match(text) {
            found.push(p.kind);
        }
    }
    found
}

pub fn contains_secret(text: &str) -> bool {
    patterns().iter().any(|p| p.re.is_match(text))
}

/// 脱敏文本：命中模式替换为 `[REDACTED:<kind>]`。
/// 未承诺识别一切形态（§12.4）——只覆盖常见模式，宁误报不漏报。
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    for p in patterns() {
        out =
            p.re.replace_all(&out, format!("[REDACTED:{}]", p.kind.label()))
                .into_owned();
    }
    out
}

/// 脱敏并返回是否发生了拦截（供 Trace 记录）。
pub fn redact_tracked(text: &str) -> (String, Vec<SecretKind>) {
    let kinds = scan(text);
    (redact(text), kinds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_key_formats() {
        assert!(contains_secret("key = sk-abc123def456ghi789jklmno"));
        assert!(contains_secret("AKIAIOSFODNN7EXAMPLE"));
        assert!(contains_secret(
            "token: ghp_abcdefghijklmnopqrstuvwxys1234567890"
        ));
        assert!(contains_secret("xoxb-123456789012-abcdefghijklmnop"));
        assert!(contains_secret("AIzaSyA-1234567890abcdefghijklmnopqrstu"));
        // GLM 风格
        assert!(contains_secret(
            "bd00e25c38114f2599384602e3c8bd44.MBDtypZkESxBINjp"
        ));
    }

    #[test]
    fn env_style_assignment_is_detected() {
        assert!(contains_secret("STRIPE_KEY=sk_live_verylongvalue"));
        assert!(contains_secret("export DATABASE_PASSWORD=hunter2hunter2"));
        // 普通赋值不是密钥
        assert!(!contains_secret("PORT=8080"));
        assert!(!contains_secret("let x = 42;"));
    }

    #[test]
    fn redaction_masks_values_and_reports_kinds() {
        let (out, kinds) = redact_tracked(
            "aws AKIAIOSFODNN7EXAMPLE and glm bd00e25c38114f2599384602e3c8bd44.MBDtypZkESxBINjp",
        );
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!out.contains("MBDtypZkESxBINjp"));
        assert!(out.contains("[REDACTED:aws_access_key]"));
        assert!(out.contains("[REDACTED:bigmodel_key]"));
        assert!(kinds.contains(&SecretKind::AwsAccessKey));
        assert!(kinds.contains(&SecretKind::BigModel));
    }

    #[test]
    fn git_diff_output_is_redacted_too() {
        // §12.4：拦截覆盖 git_read 输出
        let diff = "+stripe_key = \"sk_live_51H8xYzVerySecretKey99\" \n-context line";
        let out = redact(diff);
        assert!(!out.contains("sk_live_51H8xYzVerySecretKey99"));
        assert!(out.contains("[REDACTED:"));
        assert!(out.contains("-context line"));
    }

    #[test]
    fn private_key_block_detected() {
        assert!(contains_secret("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(contains_secret("-----BEGIN OPENSSH PRIVATE KEY-----"));
    }

    #[test]
    fn normal_code_is_untouched() {
        let code = "fn main() { let token_count = tokens.len(); }";
        assert_eq!(redact(code), code);
        assert!(!contains_secret(code));
    }
}
