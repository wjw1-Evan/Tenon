//! Laya 运行时（§9.8）：模型装载 + 三集成点 + 200ms 超时 + 整体回退。
//!
//! - 模型存放 `~/.tenon/models/laya/model.json`（§14.1），未下载即整体回退；
//! - 集成点逐项开关（`[models.laya].features`，附录 E；v1.92 收敛为
//!   intent / risk / routing 三点，预筛与批量 triage 已移除）；
//! - 推理超时默认 200ms（config 可调则由调用方传入）；
//! - 判定入 Trace 由调用方（AgentSession）以 `decider_call` 事件记录
//!   （类型 / 结果 / 耗时，不含输入原文，§14.2）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::model::LayaModel;
use crate::primitives::{Feature, IntentLabel, LayaOutcome};
use crate::Result;

/// 推理超时（§9.8：默认 200ms）。
pub const INFERENCE_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub struct LayaRuntime {
    model: Arc<Mutex<Option<Arc<LayaModel>>>>,
    models_dir: PathBuf,
    enabled_features: std::collections::HashSet<Feature>,
}

impl LayaRuntime {
    /// 打开运行时：从 `models_dir/model.json` 装载（存在时）。
    /// 加载失败不 panic → Unavailable 回退（§9.8）。
    pub fn open(models_dir: &Path, enabled_features: &[String]) -> Self {
        let model = std::fs::read(models_dir.join("model.json"))
            .ok()
            .and_then(|bytes| LayaModel::parse(&bytes).ok());
        Self {
            model: Arc::new(Mutex::new(model.map(Arc::new))),
            models_dir: models_dir.to_path_buf(),
            enabled_features: Feature::enabled_set(enabled_features),
        }
    }

    /// 模型是否已装载。
    pub async fn is_loaded(&self) -> bool {
        self.model.lock().await.is_some()
    }

    /// 已装载模型版本（§15 GET /models Laya 状态）。
    pub async fn version(&self) -> Option<(String, u32)> {
        let guard = self.model.lock().await;
        guard.as_ref().map(|m| (m.name.clone(), m.version))
    }

    /// 安装（下载 / registry 校验后）：写入 models_dir 并热装载。
    pub async fn install(&self, bytes: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.models_dir)?;
        // 安装前再验一次结构
        LayaModel::parse(bytes)?;
        std::fs::write(self.models_dir.join("model.json"), bytes)?;
        let model = LayaModel::parse(bytes)?;
        *self.model.lock().await = Some(Arc::new(model));
        Ok(())
    }

    async fn with_model<T, F>(&self, feature: Feature, f: F) -> LayaOutcome<T>
    where
        F: FnOnce(&Arc<LayaModel>) -> Option<T> + Send + 'static,
        T: Send + 'static,
    {
        if !self.enabled_features.contains(&feature) {
            return LayaOutcome::Disabled;
        }
        let model = self.model.lock().await.clone();
        let Some(model) = model else {
            return LayaOutcome::Unavailable("模型未下载");
        };
        let started = Instant::now();
        // 推理在阻塞线程执行，超时 200ms 即回退（§9.8）
        let handle = tokio::task::spawn_blocking(move || f(&model));
        match tokio::time::timeout(INFERENCE_TIMEOUT, handle).await {
            Ok(Ok(Some(value))) => {
                let duration_ms = started.elapsed().as_millis();
                let confidence = None; // 置信度未校准，仅排序参考
                LayaOutcome::Success {
                    value,
                    confidence,
                    duration_ms,
                }
            }
            Ok(Ok(None)) => LayaOutcome::Unavailable("模型无该任务头"),
            Ok(Err(_)) => LayaOutcome::Unavailable("推理线程失败"),
            Err(_) => LayaOutcome::TimedOut,
        }
    }

    /// 集成点 #1 意图预判（choice）：用户消息 → 意图标签。
    pub async fn intent(&self, user_message: impl Into<String>) -> LayaOutcome<IntentLabel> {
        let user_message = user_message.into();
        self.with_model(Feature::Intent, move |m| {
            m.classify_intent(&user_message).map(|(l, _)| l)
        })
        .await
    }

    /// 集成点 #2 命令风险辅助（score）：0..1，仅提示用，不改分级。
    pub async fn risk(&self, command: impl Into<String>) -> LayaOutcome<f32> {
        let command = command.into();
        self.with_model(Feature::Risk, move |m| m.score_risk(&command))
            .await
    }

    /// 集成点 #3 路由启发（bool）：纯读任务 → 建议轻模型（承接 §11 轻量启发式）。
    pub async fn route_suggest_light(&self, task_text: impl Into<String>) -> LayaOutcome<bool> {
        let task_text = task_text.into();
        self.with_model(Feature::Routing, move |m| {
            m.classify_intent(&task_text)
                .map(|(l, _)| matches!(l, IntentLabel::PureQa | IntentLabel::ReadOnlyAnalysis))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_with_starter(dir: &Path, features: &[&str]) -> LayaRuntime {
        let bytes = include_bytes!("../models/laya-starter-v1.json");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("model.json"), bytes).unwrap();
        let feats: Vec<String> = features.iter().map(|s| s.to_string()).collect();
        LayaRuntime::open(dir, &feats)
    }

    fn empty_runtime(features: &[&str]) -> LayaRuntime {
        let feats: Vec<String> = features.iter().map(|s| s.to_string()).collect();
        LayaRuntime::open(Path::new("/nonexistent-laya-dir"), &feats)
    }

    #[tokio::test]
    async fn intent_classifies_read_only_tasks() {
        let rt = runtime_with_starter(
            Path::new(&std::env::temp_dir().join("laya-t1")),
            &["intent", "risk", "routing"],
        );
        let out = rt.intent("解释一下这段认证流程，不要改任何文件").await;
        assert!(out.is_success(), "{out:?}");
        let label = match &out {
            LayaOutcome::Success { value, .. } => *value,
            other => panic!("{other:?}"),
        };
        assert!(
            matches!(label, IntentLabel::ReadOnlyAnalysis | IntentLabel::PureQa),
            "只读任务意图: {label:?}"
        );
    }

    #[tokio::test]
    async fn intent_classifies_fix_tasks() {
        let rt = runtime_with_starter(
            Path::new(&std::env::temp_dir().join("laya-t2")),
            &["intent", "risk", "routing"],
        );
        let out = rt.intent("修复登录接口的 bug，更新测试").await;
        assert!(
            matches!(
                &out,
                LayaOutcome::Success {
                    value: IntentLabel::NeedsChange,
                    ..
                }
            ),
            "{out:?}"
        );
    }

    #[tokio::test]
    async fn risk_scores_dangerous_commands_higher() {
        let rt = runtime_with_starter(
            Path::new(&std::env::temp_dir().join("laya-t3")),
            &["intent", "risk", "routing"],
        );
        let dangerous = rt
            .risk("rm -rf build && git push --force")
            .await
            .value()
            .unwrap();
        let safe = rt.risk("cargo test --quiet").await.value().unwrap();
        assert!(dangerous > safe, "dangerous={dangerous} safe={safe}");
    }

    #[tokio::test]
    async fn routing_suggests_light_for_read_tasks() {
        let rt = runtime_with_starter(
            Path::new(&std::env::temp_dir().join("laya-t4")),
            &["intent", "risk", "routing"],
        );
        let light = rt.route_suggest_light("总结这个模块的职责").await;
        assert!(
            matches!(&light, LayaOutcome::Success { value: true, .. }),
            "{light:?}"
        );
        let write = rt.route_suggest_light("修复 crash 并补充测试").await;
        assert!(
            matches!(&write, LayaOutcome::Success { value: false, .. }),
            "{write:?}"
        );
    }

    #[tokio::test]
    async fn disabled_feature_returns_disabled() {
        let rt = runtime_with_starter(
            Path::new(&std::env::temp_dir().join("laya-t6")),
            &["intent"],
        );
        let out = rt.risk("rm -rf /").await;
        assert!(matches!(out, LayaOutcome::Disabled), "{out:?}");
    }

    #[tokio::test]
    async fn missing_model_falls_back() {
        let rt = empty_runtime(&["intent", "risk", "routing"]);
        let out = rt.intent("修复 bug").await;
        assert!(
            matches!(out, LayaOutcome::Unavailable("模型未下载")),
            "{out:?}"
        );
        // 「未下载不阻塞」：路由启发回退 → 调用方按现状执行
    }

    #[tokio::test]
    async fn install_hot_loads_model() {
        let dir = std::env::temp_dir().join(format!("laya-t8-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let feats = vec!["intent".to_string()];
        let rt = LayaRuntime::open(&dir, &feats);
        assert!(!rt.is_loaded().await);
        let bytes = include_bytes!("../models/laya-starter-v1.json");
        rt.install(bytes).await.expect("install");
        assert!(rt.is_loaded().await);
        let out = rt.intent("修复 bug").await;
        assert_eq!(out.value(), Some(IntentLabel::NeedsChange));
    }
}
