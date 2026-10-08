// diff 面板（设计方案 §7.1 / §8.7）：万行统一 diff 虚拟滚动。
// 只物化可视窗口 + overscan，避免一次插入数万 DOM 节点导致主线程卡顿。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

const LINE_HEIGHT = 18;
const OVERSCAN_LINES = 16;
const DEFAULT_VIEWPORT_HEIGHT = 240;

/** 统一 diff 行分类；保留空行与 hunk 元数据，供渲染与未来行级跳转复用。 */
export function parseDiffLines(diff: string): string[] {
  return diff.split(/\r\n|\r|\n/);
}

function diffLineClass(line: string) {
  if (line.startsWith("+") && !line.startsWith("+++")) return "diff-row add";
  if (line.startsWith("-") && !line.startsWith("---")) return "diff-row remove";
  if (line.startsWith("@@")) return "diff-row hunk";
  return "diff-row";
}

export function DiffPanel({
  diff,
  title,
  emptyText = "(no changes)",
  viewportHeight = DEFAULT_VIEWPORT_HEIGHT,
}: {
  diff: string | null;
  title?: string;
  /** 空态占位文案；调用方经 t("diff.no_changes") 传入（随应用语言）。 */
  emptyText?: string;
  /** 测试 / 嵌入方可显式覆盖；运行态由 ResizeObserver 使用实际容器高度。 */
  viewportHeight?: number;
}) {
  const bodyRef = useRef<HTMLPreElement | null>(null);
  const frame = useRef<number | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [measuredHeight, setMeasuredHeight] = useState(viewportHeight);
  const lines = useMemo(() => (diff ? parseDiffLines(diff) : []), [diff]);
  // diff 内容变化（切换回合 / 新 diff 到达）时回到顶部：沿用旧偏移会停在
  // 与新内容无关的位置（更短的新 diff 还会被夹在尾部）
  useEffect(() => {
    setScrollTop(0);
    const node = bodyRef.current;
    if (node) node.scrollTop = 0;
  }, [diff]);
  const height = Math.max(1, measuredHeight || DEFAULT_VIEWPORT_HEIGHT);
  const visibleCount = Math.ceil(height / LINE_HEIGHT);
  const start = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN_LINES);
  const end = Math.min(lines.length, start + visibleCount + OVERSCAN_LINES * 2);
  const windowed = lines.slice(start, end);

  const scheduleScroll = useCallback((value: number) => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      setScrollTop(value);
    });
  }, []);

  useEffect(() => {
    const node = bodyRef.current;
    if (!node) return;
    if (typeof ResizeObserver === "undefined") {
      setMeasuredHeight(node.clientHeight || viewportHeight);
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const next = entries[0]?.contentRect.height ?? node.clientHeight;
      setMeasuredHeight(Math.max(1, next || viewportHeight));
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [viewportHeight]);

  useEffect(
    () => () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    },
    []
  );

  return (
    <div className="diff-panel" data-testid="diff-panel">
      <div className="diff-title">
        <span>{title ?? "diff"}</span>
        {diff && <output className="diff-range">{start + 1}–{end} / {lines.length}</output>}
      </div>
      {diff ? (
        <pre
          className="diff-body"
          data-testid="diff-body"
          tabIndex={0}
          aria-label={title ?? "diff"}
          ref={bodyRef}
          onScroll={(event) => scheduleScroll(event.currentTarget.scrollTop)}
        >
          <div className="diff-spacer" style={{ height: lines.length * LINE_HEIGHT }} />
          <div
            className="diff-window"
            style={{ transform: `translateY(${start * LINE_HEIGHT}px)` }}
          >
            {windowed.map((line, index) => (
              <div
                key={`${start + index}:${line}`}
                className={diffLineClass(line)}
                data-testid="diff-line"
                style={{ height: LINE_HEIGHT }}
              >
                {line || " "}
              </div>
            ))}
          </div>
        </pre>
      ) : (
        <pre className="diff-body muted">{emptyText}</pre>
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
