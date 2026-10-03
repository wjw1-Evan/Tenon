//! 崩溃恢复（设计方案 §10.3 / §4.2）：
//! daemon 重启时扫描非终态会话（EXECUTING / DECIDING / AWAITING_APPROVAL 等）
//! ——回滚到最近快照（EXECUTING 中崩溃不承诺原地续跑）→ 会话转 ROLLED_BACK，
//! 依据事件日志可重建上下文（事件溯源不删）。
//!
//! 不变式：会话状态 / 事件 / 上下文 100% 可恢复；「自动档 = 必可回滚」。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use tenon_snapshot::SnapshotStore;
use tenon_store::{EventKind, SessionStatus, Store};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecoveryReport {
    /// 处理的陈旧会话 id
    pub sessions: Vec<String>,
    /// 每会话回滚的文件集
    pub rolled_back_files: Vec<(String, Vec<String>)>,
    /// 恢复过程中的错误（会话 id → 原因；尽力恢复语义）
    pub errors: Vec<(String, String)>,
}

/// 恢复陈旧会话：非终态（执行链路上）且 daemon 不再持有运行时 → 判定为崩溃遗留。
pub async fn recover_stale_sessions(
    store: Arc<Mutex<Store>>,
    snapshots_root: &Path,
) -> Result<RecoveryReport, tenon_store::StoreError> {
    let mut report = RecoveryReport::default();
    let stale: Vec<(String, String)> = {
        let mut st = store.lock().await;
        let sessions = st.list_all_sessions()?;
        sessions
            .into_iter()
            .filter(|s| {
                matches!(
                    s.status,
                    SessionStatus::Idle
                        | SessionStatus::Sensing
                        | SessionStatus::Deciding
                        | SessionStatus::Executing
                        | SessionStatus::Verifying
                        | SessionStatus::Fixing
                        | SessionStatus::AwaitingApproval
                )
            })
            .map(|s| (s.id, s.project_id))
            .collect()
    };

    for (session_id, project_id) in stale {
        let mut errors: Vec<(String, String)> = Vec::new();
        let (project_path, files_rolled) = {
            let mut st = store.lock().await;
            let Some(project) = st.project(&project_id)? else {
                continue;
            };
            let cps = st.checkpoints(&session_id)?;
            // 最近恢复点：最后一个带文件集的快照（含「先快照后写入」的写前行，
            // EXECUTING 中崩溃正是要回到它）
            let target = cps.iter().rev().find(|c| !c.files.is_empty()).cloned();
            let workspace = PathBuf::from(&project.path);
            let mut rolled: Vec<String> = Vec::new();
            let mut restore_error: Option<String> = None;
            if let Some(cp) = target {
                // 快照库问题 → 尽力恢复：失败即记录（状态仍转 ROLLED_BACK 入 Trace）
                match SnapshotStore::open(snapshots_root, &project.id, &workspace, 2) {
                    Ok(snapshots) => {
                        let _safety = snapshots.snapshot();
                        match snapshots.restore(&cp.tree) {
                            Ok(()) => rolled = cp.files.clone(),
                            Err(e) => restore_error = Some(e.to_string()),
                        }
                    }
                    Err(e) => restore_error = Some(e.to_string()),
                }
            }
            if let Some(err) = restore_error {
                errors.push((session_id.clone(), err));
            }
            (workspace, rolled)
        };
        let _ = project_path;

        // 状态转 ROLLED_BACK + 恢复事件入 Trace（§14.2）
        let mut st = store.lock().await;
        let _ = st.set_session_status(&session_id, SessionStatus::RolledBack);
        let _ = st.append_event(
            &session_id,
            EventKind::Rollback,
            &serde_json::json!({
                "recovery": true,
                "files": files_rolled,
            }),
        );
        report
            .rolled_back_files
            .push((session_id.clone(), files_rolled));
        report.errors.append(&mut errors);
        report.sessions.push(session_id);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_stale_sessions_is_noop() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let dir = tempfile::tempdir().unwrap();
        let report = recover_stale_sessions(store, dir.path()).await.unwrap();
        assert!(report.sessions.is_empty());
    }
}
