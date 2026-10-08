// 统一保存（§8.2 v1.75）：自动 / 手动保存模式共用一条显式保存路径。
// 待写盘条目走 AutoSaver flush（自动保存模式的去抖内容）；否则未保存
// 缓冲直接写盘。失败保留未保存圆点，下次编辑或手动保存重试。

import type { AutoSaver } from "./autosave";

export interface SaveNowDeps {
  autosaver: AutoSaver | null;
  isUnsaved: (path: string) => boolean;
  /** 当前缓冲内容；null = tab 已不存在（跳过）。 */
  getContent: (path: string) => string | null;
  write: (path: string, content: string) => Promise<void>;
  onSaved: (path: string) => void;
  /** 写盘失败回调（v1.167 通知接入）：失败保留未保存圆点语义不变。 */
  onError?: (path: string, error: unknown) => void;
}

export async function saveNow(path: string, deps: SaveNowDeps): Promise<void> {
  if (deps.autosaver?.pending(path)) {
    await deps.autosaver.flush(path);
    return;
  }
  if (!deps.isUnsaved(path)) return;
  const content = deps.getContent(path);
  if (content === null) return;
  try {
    await deps.write(path, content);
    deps.onSaved(path);
  } catch (error) {
    // 写盘失败不回滚状态：内容仍在编辑器中，下次编辑或手动保存重试
    deps.onError?.(path, error);
  }
}
