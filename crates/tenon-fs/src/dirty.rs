//! 脏缓冲注册表（设计方案 §8.6 人机共编）：
//! UI 将未保存缓冲推送到 daemon；代理写盘前经此检查——
//! 有脏缓冲 → 三方合并（base=登记时磁盘内容，ours=代理结果，theirs=脏缓冲），
//! 合并失败 → 阻断写入并出三栏预览事件。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// 一份脏缓冲：`base` = UI 装载/最近保存时的磁盘内容；`dirty` = 当前未保存内容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirtyBuffer {
    pub base: String,
    pub dirty: String,
}

#[derive(Debug, Default)]
pub struct DirtyBufferRegistry {
    /// 项目相对路径 → 脏缓冲
    map: Mutex<HashMap<String, DirtyBuffer>>,
}

impl DirtyBufferRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记 / 更新（UI 推送未保存内容）。首次登记时 base 取当前磁盘内容。
    pub fn set(&self, path: &str, dirty_content: &str, disk_content: &str) {
        let mut map = self.map.lock().expect("dirty lock");
        let entry = map.entry(path.to_string()).or_insert_with(|| DirtyBuffer {
            base: disk_content.to_string(),
            dirty: dirty_content.to_string(),
        });
        entry.dirty = dirty_content.to_string();
    }

    pub fn get(&self, path: &str) -> Option<DirtyBuffer> {
        self.map.lock().expect("dirty lock").get(path).cloned()
    }

    /// 保存 / 关闭后解除（base 同步为新磁盘内容）。
    pub fn clear(&self, path: &str) {
        self.map.lock().expect("dirty lock").remove(path);
    }

    /// 合并完成后更新 base（脏缓冲保留，UI 继续编辑基于新基线）。
    pub fn update_base(&self, path: &str, new_base: &str) {
        if let Some(entry) = self.map.lock().expect("dirty lock").get_mut(path) {
            entry.base = new_base.to_string();
        }
    }

    pub fn is_dirty(&self, path: &str) -> bool {
        self.map.lock().expect("dirty lock").contains_key(path)
    }

    pub fn list(&self) -> Vec<String> {
        let map = self.map.lock().expect("dirty lock");
        let mut keys: Vec<String> = map.keys().cloned().collect();
        keys.sort();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_update_clear_cycle() {
        let reg = DirtyBufferRegistry::new();
        assert!(!reg.is_dirty("a.rs"));
        reg.set("a.rs", "dirty v1", "base v0");
        assert!(reg.is_dirty("a.rs"));
        let buf = reg.get("a.rs").unwrap();
        assert_eq!(buf.base, "base v0");
        assert_eq!(buf.dirty, "dirty v1");
        // 更新 dirty 不改 base
        reg.set("a.rs", "dirty v2", "should-not-overwrite");
        let buf = reg.get("a.rs").unwrap();
        assert_eq!(buf.base, "base v0");
        assert_eq!(buf.dirty, "dirty v2");
        reg.clear("a.rs");
        assert!(!reg.is_dirty("a.rs"));
    }

    #[test]
    fn update_base_after_merge() {
        let reg = DirtyBufferRegistry::new();
        reg.set("b.rs", "user edit", "original");
        reg.update_base("b.rs", "merged content");
        let buf = reg.get("b.rs").unwrap();
        assert_eq!(buf.base, "merged content");
        assert_eq!(buf.dirty, "user edit");
    }

    #[test]
    fn list_sorted() {
        let reg = DirtyBufferRegistry::new();
        reg.set("c.txt", "d", "b");
        reg.set("a.txt", "d", "b");
        assert_eq!(reg.list(), vec!["a.txt".to_string(), "c.txt".to_string()]);
    }
}
