// diff 面板（设计方案 §7.1 / M0 交付）：展示最近 AI 补丁的统一 diff。
export function DiffPanel({ diff, title }: { diff: string | null; title?: string }) {
  return (
    <div className="diff-panel" data-testid="diff-panel">
      <div className="diff-title">{title ?? "diff"}</div>
      {diff ? (
        <pre className="diff-body">{diff}</pre>
      ) : (
        <pre className="diff-body muted">（无改动）</pre>
      )}
    </div>
  );
}

/** 从 patch_applied 事件提取统一 diff（§15 patch_applied → executor 输出）。 */
export function diffFromPatchEvent(payload: Record<string, unknown>): string | null {
  const output = payload.output as { content?: string } | undefined;
  const content = output?.content ?? "";
  return content.includes("--- a/") ? content : null;
}
