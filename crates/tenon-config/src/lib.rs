//! Tenon 全局配置（设计方案附录 E schema）。
//!
//! `~/.tenon/config.toml` 不含密钥；模型密钥存系统钥匙串（§11），
//! 开发期本地联调文件 `config.local.toml` 由调用方显式加载、不入库。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION_NOTE: &str = "schema: design.md 附录 E (v1.11)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Manual,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// 首改缓冲毫秒（§9.3）
    #[serde(rename = "first_edit_buffer")]
    pub first_edit_buffer_ms: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            first_edit_buffer_ms: 2000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CircuitConfig {
    pub max_files: u32,
    pub max_lines: u64,
    pub max_tokens: u64,
    pub max_cost_usd: f64,
}

impl Default for CircuitConfig {
    fn default() -> Self {
        Self {
            max_files: 15,
            max_lines: 1500,
            max_tokens: 500_000,
            max_cost_usd: 5.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FixLoopConfig {
    pub max_rounds: u32,
    /// 无测试仓库降级通道（§9.4）
    pub low_verification_rounds: u32,
}

impl Default for FixLoopConfig {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            low_verification_rounds: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecConfig {
    /// 单条命令（测试 / 构建）超时（§9.2）
    pub command_timeout_s: u64,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            command_timeout_s: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub circuit: CircuitConfig,
    pub fix_loop: FixLoopConfig,
    pub exec: ExecConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::default_with(CircuitConfig::default())
    }
}

impl AgentConfig {
    pub fn default_with(circuit: CircuitConfig) -> Self {
        Self {
            circuit,
            fix_loop: FixLoopConfig::default(),
            exec: ExecConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckpointConfig {
    /// shadow 库 gc prune 天数（§10.3 v1.93；0 = 不清理）
    pub keep_days: u32,
    /// 未跟踪大文件排除阈值 MB（§10.3）
    pub max_untracked_mb: u64,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            keep_days: 7,
            max_untracked_mb: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    #[default]
    Seatbelt,
    LandlockSeccomp,
    Wsl2,
    /// Windows 无 WSL2 降级档：仅 A 级 + 写守卫、强制交互档（§12.3）
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    pub macos: SandboxBackend,
    pub linux: SandboxBackend,
    pub windows: SandboxBackend,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            macos: SandboxBackend::Seatbelt,
            linux: SandboxBackend::LandlockSeccomp,
            windows: SandboxBackend::Wsl2,
        }
    }
}

/// AI Evals 流水线（§18.3 / M3）：定时自动触发基准套件。
/// 派生默认 `{ interval_hours: 0, provider: "" }`——0 表示关闭定时触发，
/// 手动 `tenon-evals` 仍可用。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EvalsConfig {
    /// 自动触发间隔（小时）；0 = 关闭（默认；手动 tenon-evals 仍可用）
    pub interval_hours: u32,
    /// 套件 provider（空 = 跟随 models.default）
    pub provider: String,
}

/// 多项目运行模型（§6.4 / ADR-15）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectsConfig {
    /// 同时打开项目上限。
    pub max_open: usize,
    /// 跨项目同时 EXECUTING 的全局上限；项目内仍受写锁约束。
    pub max_concurrent_agent_tasks: usize,
    /// ProjectRuntime 引用归零后的空闲回收延迟。
    pub idle_runtime_ttl_seconds: u64,
    /// 最近项目列表保留数。
    /// 显式允许打开 monorepo + 子包等嵌套根；仍按 project_id 隔离。
    pub allow_linked_workspace: bool,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            max_open: 12,
            max_concurrent_agent_tasks: 2,
            idle_runtime_ttl_seconds: 600,
            allow_linked_workspace: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveConfig {
    /// §14.2 增长治理：热数据保留天数
    pub events_days: u32,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self { events_days: 90 }
    }
}

/// 模型 provider 协议族（v1.11）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI Chat Completion 兼容（含 DeepSeek / Ollama / GLM 等）
    #[default]
    Openai,
    /// Anthropic Messages 协议
    Anthropic,
    /// OpenAI Responses 协议
    OpenaiResponses,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderConfig {
    /// 协议族；缺省按 provider 名推断（ollama → openai）
    pub kind: Option<ProviderKind>,
    pub base_url: String,
    /// chat | responses（§3.1 schema 借鉴 codex）
    pub wire_api: Option<String>,
    /// 钥匙串引用的环境变量名，不落盘密钥
    pub api_key_env: Option<String>,
    /// 开发期直填密钥（仅 config.local.toml git-ignore 文件；正式配置走钥匙串）
    pub api_key: Option<String>,
    /// 该 provider 默认模型（v1.11）
    pub model: Option<String>,
    /// 美元 / 百万输入 token（v1.93 §11；缺省 = 未定价不计成本）
    pub price_in_per_mtok: Option<f64>,
    /// 美元 / 百万输出 token
    pub price_out_per_mtok: Option<f64>,
}

/// Laya 本地决策模型（§9.8）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayaConfig {
    /// 总开关；false = 各集成点回退现状
    pub enabled: bool,
    pub auto_download: bool,
    /// cpu；gpu 预留
    /// 集成点逐项开关（§9.8 表 #1-3，v1.92 收敛）
    pub features: Vec<String>,
}

impl Default for LayaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_download: true,
            features: vec!["intent".into(), "risk".into(), "routing".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelsConfig {
    /// 空 = 首次启动引导选择（§11）
    pub default: String,
    pub providers: std::collections::BTreeMap<String, ProviderConfig>,
    pub laya: LayaConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub channel: UpdateChannel,
    /// 静态签名更新清单（v1.86）；manual 模式只有显式检查才访问。
    pub manifest_url: String,
    /// ed25519 公钥（hex，32 bytes）；空串 = updater fail closed。
    pub public_key_hex: String,
    /// auto 周期检查间隔；0 禁用。
    pub check_interval_s: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::Manual,
            manifest_url: "https://tenonide.dev/updates/manifest.json".into(),
            public_key_hex: String::new(),
            check_interval_s: 21_600,
        }
    }
}

/// 全局配置（附录 E）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub update: UpdateConfig,
    pub session: SessionConfig,
    pub agent: AgentConfig,
    pub checkpoint: CheckpointConfig,
    pub archive: ArchiveConfig,
    pub evals: EvalsConfig,
    pub projects: ProjectsConfig,
    pub models: ModelsConfig,
}

impl Config {
    /// 解析 TOML 文本；未列出的字段全部取设计默认值。
    pub fn parse_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// 从文件加载（开发期 config.local.toml / 全局 config.toml 共用）。
    pub fn parse_toml_file(path: &Path) -> std::result::Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(path.to_path_buf(), e.to_string()))?;
        Self::parse_toml(&text).map_err(|e| ConfigError::Parse(path.to_path_buf(), e.to_string()))
    }

    /// 从 `~/.tenon/config.toml` 加载；文件不存在时返回默认值。
    pub fn load_global() -> Result<Self, ConfigError> {
        let path = Self::global_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| ConfigError::Io(path.clone(), e.to_string()))?;
        Self::parse_toml(&text).map_err(|e| ConfigError::Parse(path.clone(), e.to_string()))
    }

    pub fn global_path() -> PathBuf {
        Self::data_dir().join("config.toml")
    }

    /// 本地数据目录 `~/.tenon/`（§14.1）。
    pub fn data_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".tenon")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("读取配置失败 {0}: {1}")]
    Io(PathBuf, String),
    #[error("解析配置失败 {0}: {1}")]
    Parse(PathBuf, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_EXAMPLE: &str = r#"
update.channel  = "manual"

[session]
first_edit_buffer = 1500

[agent.circuit]
max_files   = 20
max_lines   = 3000
max_tokens  = 800000
max_cost_usd = 10.0

[agent.fix_loop]
max_rounds              = 2
low_verification_rounds = 1

[agent.exec]
command_timeout_s = 60

[checkpoint]
keep_days        = 3
max_untracked_mb = 5

[archive]
events_days = 30

[models]
default = "glm"

[models.providers.glm]
kind     = "anthropic"
base_url = "https://open.bigmodel.cn/api/anthropic"
model    = "glm-4.6"

[models.providers.ollama]
base_url    = "http://127.0.0.1:11434/v1"
wire_api    = "chat"
api_key_env = ""

[models.laya]
enabled       = false
auto_download = true
device        = "cpu"
features      = ["intent", "risk"]
"#;

    #[test]
    fn parses_full_example_with_defaults_for_missing() {
        let cfg = Config::parse_toml(FULL_EXAMPLE).expect("parse");
        assert_eq!(cfg.session.first_edit_buffer_ms, 1500);
        assert_eq!(cfg.agent.circuit.max_files, 20);
        assert_eq!(cfg.agent.circuit.max_lines, 3000);
        assert_eq!(cfg.agent.fix_loop.max_rounds, 2);
        assert_eq!(cfg.agent.exec.command_timeout_s, 60);
        assert_eq!(cfg.checkpoint.keep_days, 3);
        assert_eq!(cfg.archive.events_days, 30);
        assert_eq!(cfg.models.default, "glm");

        let glm = cfg.models.providers.get("glm").expect("glm provider");
        assert_eq!(glm.kind, Some(ProviderKind::Anthropic));
        assert_eq!(glm.model.as_deref(), Some("glm-4.6"));

        let ollama = cfg.models.providers.get("ollama").expect("ollama provider");
        // kind 缺省 → OpenAI 兼容
        assert_eq!(ollama.kind, None);
        assert_eq!(ollama.base_url, "http://127.0.0.1:11434/v1");

        assert!(!cfg.models.laya.enabled);
        assert_eq!(
            cfg.models.laya.features,
            vec!["intent".to_string(), "risk".to_string()]
        );
    }

    #[test]
    fn empty_input_yields_design_defaults() {
        let cfg = Config::parse_toml("").expect("parse empty");
        assert_eq!(cfg.update.channel, UpdateChannel::Manual);
        assert_eq!(cfg.session.first_edit_buffer_ms, 2000);
        assert_eq!(cfg.agent.circuit.max_files, 15);
        assert_eq!(cfg.agent.circuit.max_lines, 1500);
        assert_eq!(cfg.agent.circuit.max_tokens, 500_000);
        assert_eq!(cfg.agent.circuit.max_cost_usd, 5.0);
        assert_eq!(cfg.agent.fix_loop.max_rounds, 3);
        assert_eq!(cfg.agent.fix_loop.low_verification_rounds, 1);
        assert_eq!(cfg.agent.exec.command_timeout_s, 120);
        assert_eq!(cfg.checkpoint.keep_days, 7);
        assert_eq!(cfg.checkpoint.max_untracked_mb, 2);
        assert_eq!(cfg.archive.events_days, 90);
        assert_eq!(cfg.projects.max_open, 12);
        assert_eq!(cfg.projects.max_concurrent_agent_tasks, 2);
        assert_eq!(cfg.projects.idle_runtime_ttl_seconds, 600);
        assert!(!cfg.projects.allow_linked_workspace);
        assert!(cfg.models.default.is_empty());
        assert!(cfg.models.laya.enabled);
        assert!(cfg.models.laya.auto_download);
        assert_eq!(cfg.models.laya.features.len(), 3);
    }

    #[test]
    fn roundtrips_through_toml() {
        let cfg = Config::parse_toml(FULL_EXAMPLE).expect("parse");
        let text = toml::to_string(&cfg).expect("serialize");
        let cfg2 = Config::parse_toml(&text).expect("re-parse");
        assert_eq!(
            cfg.agent.circuit.max_cost_usd,
            cfg2.agent.circuit.max_cost_usd
        );
        assert_eq!(cfg.models.laya.features, cfg2.models.laya.features);
    }
}
