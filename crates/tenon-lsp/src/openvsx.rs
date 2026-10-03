//! Open VSX 语言子集实验兼容（设计方案 §13.3 / §4.1 M2）：
//! 仅声明 `contributes.languages`（languageContribution）的扩展转内部语言包。
//!
//! 实验边界（§13.3：其余不支持）：
//! - 语言服务器的启动命令取自扩展清单的实验扩展点
//!   `contributes.tenonLsp: { command, args }`——VS Code API 未标准化清单式
//!   LSP 声明，故该子集要求此 tenon 扩展点；缺失则转换失败并明示原因；
//! - 转换产物为动态语言包（运行期注册进 pack 选择器），经共享 LSP 宿主的
//!   铁律七守卫接入，与内置语言包同一安全边界。

use serde::Deserialize;
use std::sync::Mutex;

use crate::pack::LanguagePack;

#[derive(Debug, thiserror::Error)]
pub enum OpenVsxError {
    #[error("扩展清单解析失败: {0}")]
    Parse(String),
    #[error("无 languageContribution（contributes.languages 为空）")]
    NoLanguages,
    #[error("缺少语言服务器声明（contributes.tenonLsp）——Open VSX 子集要求显式 command")]
    NoServer,
}

/// Open VSX / VS Code 扩展清单子集。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OpenVsxManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub contributes: Contributions,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Contributions {
    /// languageContribution：{ id, extensions, ... }
    #[serde(default, rename = "languages")]
    pub languages: Vec<LanguageContribution>,
    /// tenon 实验扩展点：语言服务器启动命令（Open VSX 子集要求）。
    #[serde(default, rename = "tenonLsp")]
    pub tenon_lsp: Option<TenonLsp>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LanguageContribution {
    pub id: String,
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TenonLsp {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// 转换产物：可注册进 pack 选择器的动态语言包。
#[derive(Debug, Clone)]
pub struct ConvertedPack {
    /// 首个 language id（作为 LSP languageId）
    pub language: String,
    pub pack: LanguagePack,
    /// 扩展标识（id@version）
    pub extension: String,
}

/// 解析扩展清单 → 动态语言包（实验子集转换，§13.3）。
pub fn convert_extension(manifest_json: &str) -> Result<ConvertedPack, OpenVsxError> {
    let m: OpenVsxManifest =
        serde_json::from_str(manifest_json).map_err(|e| OpenVsxError::Parse(e.to_string()))?;
    let first = m
        .contributes
        .languages
        .first()
        .ok_or(OpenVsxError::NoLanguages)?;
    let lsp = m
        .contributes
        .tenon_lsp
        .as_ref()
        .ok_or(OpenVsxError::NoServer)?;

    let mut extensions: Vec<String> = Vec::new();
    for lang in &m.contributes.languages {
        for e in &lang.extensions {
            let e = e.trim_start_matches('.').to_lowercase();
            if !extensions.contains(&e) {
                extensions.push(e);
            }
        }
    }

    // 语言包为长生命周期对象：字符串与切片 leak 成 'static
    //（转换按扩展注册去重，量级有限；与内置包 &'static 形态对齐）
    let extensions_static: Vec<&'static str> = extensions
        .into_iter()
        .map(|e| Box::leak(e.into_boxed_str()) as &'static str)
        .collect();
    let pack = LanguagePack {
        language: Box::leak(first.id.clone().into_boxed_str()),
        command: lsp.command.clone(),
        args: lsp.args.clone(),
        extensions: Box::leak(extensions_static.into_boxed_slice()),
        detect_files: Box::leak(Vec::new().into_boxed_slice()),
    };
    Ok(ConvertedPack {
        language: first.id.clone(),
        pack,
        extension: format!("{}@{}", m.name, m.version),
    })
}

// ---------- 动态语言包注册（运行期扩展 Open VSX 转换产物） ----------

static DYNAMIC_PACKS: Mutex<Vec<LanguagePack>> = Mutex::new(Vec::new());

/// 注册动态语言包（Open VSX 转换产物；优先于内置包匹配）。
pub fn register_dynamic_pack(pack: LanguagePack) {
    let mut packs = DYNAMIC_PACKS.lock().expect("dynamic packs lock");
    if !packs.iter().any(|p| p.language == pack.language) {
        packs.push(pack);
    }
}

/// 动态包查询（pack_for_file 在内置包未命中时回落至此）。
pub fn dynamic_pack_for_file(file: &str) -> Option<LanguagePack> {
    let ext = std::path::Path::new(file)
        .extension()?
        .to_str()?
        .to_lowercase();
    DYNAMIC_PACKS
        .lock()
        .expect("dynamic packs lock")
        .iter()
        .find(|p| p.extensions.contains(&ext.as_str()))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "name": "zomb-lang",
  "version": "0.2.0",
  "publisher": "acme",
  "contributes": {
    "languages": [
      { "id": "zomb", "extensions": [".zomb", ".zb"] }
    ],
    "tenonLsp": { "command": "zomb-lsp", "args": ["--stdio"] }
  }
}"#;

    #[test]
    fn converts_language_contribution_to_pack() {
        let converted = convert_extension(SAMPLE).unwrap();
        assert_eq!(converted.language, "zomb");
        assert_eq!(converted.pack.command, "zomb-lsp");
        assert!(converted.pack.extensions.contains(&"zomb"));
        assert!(converted.pack.extensions.contains(&"zb"));
        assert_eq!(converted.extension, "zomb-lang@0.2.0");
    }

    #[test]
    fn missing_server_or_languages_rejected() {
        let no_server =
            r#"{"name":"x","contributes":{"languages":[{"id":"z","extensions":[".z"]}]}}"#;
        assert!(matches!(
            convert_extension(no_server),
            Err(OpenVsxError::NoServer)
        ));
        let no_lang = r#"{"name":"x","contributes":{"tenonLsp":{"command":"c"}}}"#;
        assert!(matches!(
            convert_extension(no_lang),
            Err(OpenVsxError::NoLanguages)
        ));
        let bad = "{not json";
        assert!(matches!(
            convert_extension(bad),
            Err(OpenVsxError::Parse(_))
        ));
    }

    #[test]
    fn dynamic_pack_resolves_after_registration() {
        let converted = convert_extension(SAMPLE).unwrap();
        register_dynamic_pack(converted.pack.clone());
        // pack_for_file 由 manager 侧组合；此处验证动态查询
        let hit = dynamic_pack_for_file("demo.zomb").unwrap();
        assert_eq!(hit.command, "zomb-lsp");
        assert!(dynamic_pack_for_file("demo.unknown").is_none());
    }
}
