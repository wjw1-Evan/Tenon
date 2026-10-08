// 自动保存（§8.2）：编辑停顿后去抖写盘；flush 立即保存；失败保留待重试。
// 与 §8.6 脏缓冲推送独立——putBuffer 保持共编窗口，本模块负责落盘。

export type SaveFn = (path: string, content: string) => Promise<void>;

interface Entry {
  timer: number;
  content: string;
}

export interface AutoSaver {
  /** 记录一次编辑：重置该文件的去抖计时（delayMs 后写盘）。 */
  schedule: (path: string, content: string) => void;
  /** 立即保存该文件（取消去抖）；无待保存内容时为空操作。 */
  flush: (path: string) => Promise<void>;
  /** 立即保存全部待写条目（项目切换前调用：saver 回调按当前活动项目解析目标）。 */
  flushAll: () => Promise<void>;
  /** 取消该文件的去抖写盘（文件被删除/重命名后，旧路径的待写会把已删文件「复活」）。 */
  cancel: (path: string) => void;
  /** 该文件是否有待写盘的编辑。 */
  pending: (path: string) => boolean;
  /** 释放全部计时器（卸载时）。 */
  dispose: () => void;
}

export function createAutoSaver(
  save: SaveFn,
  delayMs: number,
  /** 写盘失败回调（v1.167 通知接入）：状态语义不变——内容留在编辑器、待重试。 */
  onError?: (path: string, error: unknown) => void
): AutoSaver {
  const pending = new Map<string, Entry>();

  function fire(path: string) {
    const entry = pending.get(path);
    if (!entry) return;
    pending.delete(path);
    window.clearTimeout(entry.timer);
    // 保存失败不回滚状态：内容仍在编辑器中，下次编辑 / 手动保存重试
    void save(path, entry.content).catch((error) => {
      onError?.(path, error);
    });
  }

  return {
    schedule(path, content) {
      const prev = pending.get(path);
      if (prev) window.clearTimeout(prev.timer);
      const timer = window.setTimeout(() => fire(path), delayMs);
      pending.set(path, { timer, content });
    },
    async flush(path) {
      const entry = pending.get(path);
      if (!entry) return;
      pending.delete(path);
      window.clearTimeout(entry.timer);
      await save(path, entry.content);
    },
    async flushAll() {
      const paths = [...pending.keys()];
      await Promise.all(paths.map((p) => this.flush(p)));
    },
    cancel(path) {
      const entry = pending.get(path);
      if (!entry) return;
      pending.delete(path);
      window.clearTimeout(entry.timer);
    },
    pending(path) {
      return pending.has(path);
    },
    dispose() {
      for (const { timer } of pending.values()) window.clearTimeout(timer);
      pending.clear();
    },
  };
}
