//! L5 跨会话对话记忆存储层测试（§10.1 v1.104）。
//!
//! 独立集成测试文件：memories 全链路（写入 / 去重合并 / 作用域可见性 /
//! 上限治理 / 删除）只走公开 API，不依赖 lib.rs 内部测试夹具。

use tenon_store::{MemoryRecord, Store};

fn mem() -> Store {
    Store::open_in_memory().unwrap()
}

fn mem_rec(scope: &str, kind: &str, content: &str, emb: &[f32]) -> MemoryRecord {
    MemoryRecord {
        scope: scope.into(),
        project_id: if scope == "project" {
            "p1".into()
        } else {
            String::new()
        },
        kind: kind.into(),
        content: content.into(),
        importance: 3,
        embedding: emb.to_vec(),
        source_session: "s1".into(),
    }
}

/// v1.104：写入与本地 embedding 余弦去重合并——同义记忆刷新既有条目不新增行。
#[test]
fn upsert_dedupes_similar() {
    let mut s = mem();
    let emb = [1.0f32, 0.0, 0.0];
    let (first, merged) = s
        .upsert_memory(
            &mem_rec("project", "preference", "commit message 用中文", &emb),
            0.90,
        )
        .unwrap();
    assert!(!merged);

    let (updated, merged) = s
        .upsert_memory(
            &mem_rec("project", "preference", "提交信息一律使用中文", &emb),
            0.90,
        )
        .unwrap();
    assert!(merged, "同义记忆合并到既有条目");
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.content, "提交信息一律使用中文");
    assert_eq!(s.list_memories("p1", None, 100).unwrap().len(), 1);

    // 不同向量（余弦 0）不去重，新增行
    let other = [0.0f32, 1.0, 0.0];
    let (_, merged) = s
        .upsert_memory(&mem_rec("project", "fact", "测试跑 pnpm", &other), 0.90)
        .unwrap();
    assert!(!merged);
    assert_eq!(s.list_memories("p1", None, 100).unwrap().len(), 2);
}

/// v1.104：作用域与类型校验——global 仅 preference；列表跨项目互不可见、
/// q 过滤生效、删除幂等。
#[test]
fn scope_rules_and_visibility() {
    let mut s = mem();
    // global + 非 preference 组合拒绝
    assert!(s
        .upsert_memory(&mem_rec("global", "fact", "非法组合", &[1.0, 0.0]), 0.9)
        .is_err());
    // 未知 kind 拒绝
    assert!(s
        .upsert_memory(
            &mem_rec("project", "unknown-kind", "非法类型", &[1.0, 0.0]),
            0.9
        )
        .is_err());
    // 空内容拒绝
    let mut empty = mem_rec("project", "fact", "  ", &[1.0, 0.0]);
    empty.project_id = "p1".into();
    assert!(s.upsert_memory(&empty, 0.9).is_err());

    let mut global_rec = mem_rec("global", "preference", "回复用中文", &[1.0, 0.0]);
    global_rec.project_id = String::new();
    s.upsert_memory(&global_rec, 0.9).unwrap();
    let (proj_mem, _) = s
        .upsert_memory(
            &mem_rec("project", "workflow", "本项目跑 cargo test", &[0.0, 1.0]),
            0.9,
        )
        .unwrap();

    // 项目列表 = 项目层 + global preference
    assert_eq!(s.list_memories("p1", None, 100).unwrap().len(), 2);
    // 其他项目只见 global preference
    let other = s.list_memories("p2", None, 100).unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].scope, "global");
    // q 过滤
    assert_eq!(s.list_memories("p1", Some("cargo"), 100).unwrap().len(), 1);
    // 删除存在 / 不存在
    assert!(s.delete_memory(&proj_mem.id).unwrap());
    assert!(!s.delete_memory(&proj_mem.id).unwrap());
}

/// v1.104：每项目 active 上限治理——超限按 importance 升序 + 最旧淘汰。
#[test]
fn prune_keeps_high_importance() {
    let mut s = mem();
    for (i, (importance, emb)) in [
        (5, [1.0f32, 0.0, 0.0]),
        (1, [0.0, 1.0, 0.0]),
        (3, [0.0, 0.0, 1.0]),
    ]
    .into_iter()
    .enumerate()
    {
        let mut rec = mem_rec("project", "fact", &format!("记忆{i}"), &emb);
        rec.importance = importance;
        s.upsert_memory(&rec, 0.99).unwrap();
    }
    assert_eq!(s.prune_memories("p1", 2).unwrap(), 1);
    let left = s.list_memories("p1", None, 100).unwrap();
    assert_eq!(left.len(), 2);
    assert!(
        left.iter().all(|m| m.importance > 1),
        "最低 importance 被淘汰"
    );
    assert_eq!(s.prune_memories("p1", 2).unwrap(), 0, "未超限不删");
}

/// v1.104：importance 取写入值与既有值的较大者（MAX 合并语义）。
#[test]
fn merge_keeps_max_importance() {
    let mut s = mem();
    let mut low = mem_rec("project", "decision", "采用方案 A", &[1.0f32, 0.0, 0.0]);
    low.importance = 2;
    let (first, _) = s.upsert_memory(&low, 0.9).unwrap();
    assert_eq!(first.importance, 2);

    let mut high = mem_rec("project", "decision", "最终采用方案 A", &[1.0f32, 0.0, 0.0]);
    high.importance = 5;
    let (merged_mem, merged) = s.upsert_memory(&high, 0.9).unwrap();
    assert!(merged);
    assert_eq!(merged_mem.id, first.id);
    assert_eq!(merged_mem.importance, 5, "合并取较大 importance");
}
