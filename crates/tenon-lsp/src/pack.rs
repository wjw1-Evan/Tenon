//! 内置语言包（设计方案 §8.4 内置档）：按项目探测语言并映射语言服务器。
//!
//! | 语言包 | 服务器 | 探测 | 网络 |
//! |---|---|---|---|
//! | TypeScript/JS | typescript-language-server --stdio | package.json | offline |
//! | Python | pyright-langserver --stdio | pyproject.toml / requirements.txt / *.py | offline |
//! | Rust | rust-analyzer（如已安装） | Cargo.toml | open |
//!
//! 网络策略（v2.74，design-v2.md §4.3）：纯本地分析器收紧为 offline（tsserver /
//! pyright 不出网），仅需工具链联网者（rust-analyzer 的 `cargo metadata`）保持 open。
//!
//! 运行时缺失（服务器未安装）→ 返回明确错误（安装向导随 M1 语言包体系补全）。

use std::path::Path;

/// 语言包定义。
#[derive(Debug, Clone)]
pub struct LanguagePack {
    /// 语言标识（LSP languageId 族）。
    pub language: &'static str,
    /// 服务器命令。
    pub command: String,
    /// 服务器参数。
    pub args: Vec<String>,
    /// 服务的文件扩展名。
    pub extensions: &'static [&'static str],
    /// 项目探测文件。
    pub detect_files: &'static [&'static str],
    /// 网络策略（v2.74 逐语言包收紧，design-v2.md §4.3）：纯本地分析器
    /// offline；rust-analyzer 等需工具链联网者 Open。
    pub network: tenon_sandbox::ExternalNetwork,
}

/// 内置语言包清单（§8.4 内置档；C#/Rust/Go 一键安装随 M1 后段）。
pub fn builtin_packs() -> Vec<LanguagePack> {
    vec![
        LanguagePack {
            language: "typescript",
            command: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
            extensions: &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
            detect_files: &["package.json"],
            // tsserver 纯本地分析，无需出网
            network: tenon_sandbox::ExternalNetwork::Offline,
        },
        LanguagePack {
            language: "python",
            command: "pyright-langserver".into(),
            args: vec!["--stdio".into()],
            extensions: &["py", "pyi"],
            detect_files: &["pyproject.toml", "requirements.txt", "setup.py"],
            // pyright 静态分析本地源码，无需出网
            network: tenon_sandbox::ExternalNetwork::Offline,
        },
        LanguagePack {
            language: "rust",
            command: "rust-analyzer".into(),
            args: vec![],
            extensions: &["rs"],
            detect_files: &["Cargo.toml"],
            // rust-analyzer 内部跑 cargo metadata，需拉取 registry 索引
            network: tenon_sandbox::ExternalNetwork::Open,
        },
    ]
}

/// 为文件扩展名选择语言包（含探测：项目根须有对应标记文件，§8.4 项目感知推荐）。
pub fn pack_for_file(project_root: &Path, file: &str) -> Option<LanguagePack> {
    let ext = Path::new(file).extension()?.to_str()?.to_lowercase();
    let packs = builtin_packs();
    let mut candidates: Vec<LanguagePack> = packs
        .into_iter()
        .filter(|p| p.extensions.contains(&ext.as_str()))
        .collect();
    // 项目标记文件优先（无标记也放行：单文件场景仍可用）
    candidates.sort_by_key(|p| !p.detect_files.iter().any(|f| project_root.join(f).exists()));
    candidates.into_iter().next()
}

/// LSP languageId（按扩展细分）。
pub fn language_id_for(file: &str) -> &'static str {
    let ext = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "jsx" => "javascriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "py" | "pyi" => "python",
        "rs" => "rust",
        _ => "plaintext",
    }
}

/// file:// URI（项目相对路径 → 绝对 URI）。
/// 完整 percent-encode（非保留字符外全部转义）：LSP 规范要求服务器对非
/// ASCII / 保留字符编码；只转义 `%` 与空格会让本地构造的键与服务器推送的
/// 诊断键不相等——非 ASCII 路径文件的诊断永远查不到。
pub fn file_uri(project_root: &Path, rel: &str) -> String {
    let abs = project_root.join(rel);
    let abs = std::fs::canonicalize(&abs).unwrap_or(abs);
    format!("file://{}", percent_encode_path(&abs.to_string_lossy()))
}

/// 路径成分 percent-encode（RFC 3986 非保留字符 + `/` 分隔符保留）。
pub fn percent_encode_path(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_selection_by_extension_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let pack = pack_for_file(dir.path(), "src/index.ts").unwrap();
        assert_eq!(pack.language, "typescript");
        assert_eq!(pack.command, "typescript-language-server");

        let dir2 = tempfile::tempdir().unwrap();
        std::fs::write(dir2.path().join("pyproject.toml"), "[project]\n").unwrap();
        let pack2 = pack_for_file(dir2.path(), "main.py").unwrap();
        assert_eq!(pack2.language, "python");
        assert_eq!(pack2.command, "pyright-langserver");
    }

    #[test]
    fn unknown_extension_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(pack_for_file(dir.path(), "readme.md").is_none());
    }

    #[test]
    fn builtin_packs_declare_network_policy() {
        // v2.74 §4.3：纯本地分析器 offline，需工具链联网者 open
        use tenon_sandbox::ExternalNetwork;
        let packs = builtin_packs();
        let policy = |lang: &str| {
            packs
                .iter()
                .find(|p| p.language == lang)
                .unwrap_or_else(|| panic!("缺语言包 {lang}"))
                .network
        };
        assert_eq!(policy("typescript"), ExternalNetwork::Offline);
        assert_eq!(policy("python"), ExternalNetwork::Offline);
        assert_eq!(policy("rust"), ExternalNetwork::Open);
    }

    #[test]
    fn language_ids_follow_lsp() {
        assert_eq!(language_id_for("a.ts"), "typescript");
        assert_eq!(language_id_for("a.tsx"), "typescriptreact");
        assert_eq!(language_id_for("a.js"), "javascript");
        assert_eq!(language_id_for("a.py"), "python");
        assert_eq!(language_id_for("a.rs"), "rust");
    }

    #[test]
    fn file_uri_escapes_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("my proj");
        std::fs::create_dir_all(&sub).unwrap();
        let uri = file_uri(dir.path(), "my proj/a.rs");
        assert!(uri.starts_with("file://"));
        assert!(!uri.contains(" my "), "空格须转义: {uri}");
    }
}
