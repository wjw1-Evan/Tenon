// 草稿持久化（§7.2 v1.177）：会话输入草稿防抖写 localStorage，刷新 / 重启恢复。
// 单条 ≤20k 字符、每表 ≤50 条（写入序淘汰最旧）防无界膨胀；解析失败 / 配额满
// 静默回退——草稿持久化是尽力而为的便利，不做任何提示。

const KEY = "tenon:drafts";
const MAX_ENTRY_CHARS = 20_000;
const MAX_ENTRIES = 50;

export interface DraftStore {
  /** 草稿态 per-project 文本（v1.126 多项目互不串扰语义的持久化）。 */
  drafts: Record<string, string>;
  /** 会话态 per-session 输入缓冲。 */
  sessions: Record<string, string>;
}

export function loadDrafts(): DraftStore {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { drafts: {}, sessions: {} };
    const parsed = JSON.parse(raw) as Partial<DraftStore>;
    const table = (v: unknown): Record<string, string> =>
      typeof v === "object" && v !== null && !Array.isArray(v)
        ? (Object.fromEntries(
            Object.entries(v as Record<string, unknown>).filter(
              ([, text]) => typeof text === "string",
            ),
          ) as Record<string, string>)
        : {};
    return { drafts: table(parsed.drafts), sessions: table(parsed.sessions) };
  } catch {
    return { drafts: {}, sessions: {} };
  }
}

export function saveDrafts(store: DraftStore): void {
  const prune = (table: Record<string, string>): Record<string, string> =>
    Object.fromEntries(
      Object.entries(table)
        .filter(([, text]) => text.length > 0 && text.length <= MAX_ENTRY_CHARS)
        .slice(-MAX_ENTRIES),
    );
  try {
    localStorage.setItem(
      KEY,
      JSON.stringify({ drafts: prune(store.drafts), sessions: prune(store.sessions) }),
    );
  } catch {
    // 配额满 / 隐私模式：草稿不持久化即可，不提示
  }
}

/** 测试辅助：清空持久层。 */
export function clearDrafts(): void {
  try {
    localStorage.removeItem(KEY);
  } catch {
    // ignore
  }
}
