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
  /** 该文件是否有待写盘的编辑。 */
  pending: (path: string) => boolean;
  /** 释放全部计时器（卸载时）。 */
  dispose: () => void;
}

export function createAutoSaver(save: SaveFn, delayMs: number): AutoSaver {
  const pending = new Map<string, Entry>();

  function fire(path: string) {
    const entry = pending.get(path);
    if (!entry) return;
    pending.delete(path);
    window.clearTimeout(entry.timer);
    // 保存失败不回滚状态：内容仍在编辑器中，下次编辑 / 手动保存重试
    void save(path, entry.content).catch(() => {});
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
    pending(path) {
      return pending.has(path);
    },
    dispose() {
      for (const { timer } of pending.values()) window.clearTimeout(timer);
      pending.clear();
    },
  };
}
