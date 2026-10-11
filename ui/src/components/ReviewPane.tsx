// v2.4 常驻审查面（design-v2.md §6「审查面是第二主区」）：会话运行中自动停靠
// 右栏，实时呈现代理改动 diff 流——「人改与 AI 改实时互见」从口号变默认形态。
// 空闲自动收起；用户手动关闭后不再自动弹出（会话切换重置）。
// v2.7 diff 级反馈回路：diff 行点选（DiffPanel selectable）→ 反馈卡
// （文件:行上下文 + 反馈输入）→ 任务文本经行内指令同款链路注入当前会话；
// 运行态发送自动入队（v1.147），回合自然完成后续跑消费——「边跑边审、审完即改」。
import { useMemo, useState } from "react";
import { DiffPanel, parseDiffLines } from "./DiffPanel";
import { mapDiffLines, selectionTargetFromInfos, type DiffSelectionTarget } from "../lib/diffSelect";
import type { Translate } from "../lib/i18n";

/** 审查反馈任务文本组装（design.md §7.2 v2.7）：反馈前缀 + 目标行区 + diff
 *  摘录 + 最小改动约束；文案随应用语言，摘录上限与行内指令一致（1200 字）。 */
export function buildReviewTask(
  instruction: string,
  target: DiffSelectionTarget,
  t: Translate
): string {
  const range =
    target.startLine !== undefined && target.endLine !== undefined
      ? t("review.fb_task_range", { start: target.startLine, end: target.endLine })
      : t("review.fb_task_file");
  return [
    t("review.fb_task_prefix", { path: target.path, range, instruction: instruction.trim() }),
    `${t("review.fb_task_excerpt")}\n\`\`\`diff\n${target.excerpt.trim().slice(0, 1200)}\n\`\`\``,
    t("review.fb_task_constraint"),
  ].join("\n");
}

export function ReviewPane({
  t,
  diff,
  running,
  statusLabel,
  onClose,
  onFeedback,
}: {
  t: Translate;
  diff: string | null;
  running: boolean;
  statusLabel: string;
  onClose: () => void;
  /** v2.7 反馈发送：入参为组装完成的任务文本（App 经行内指令同款链路注入会话）。 */
  onFeedback?: (task: string) => void;
}) {
  const [range, setRange] = useState<{ start: number; end: number } | null>(null);
  const [instruction, setInstruction] = useState("");
  // 行序与映射各自 memo：行序即渲染序（含空行），映射只在 diff 变化时重算
  const lines = useMemo(() => (diff ? parseDiffLines(diff) : []), [diff]);
  const infos = useMemo(() => (diff ? mapDiffLines(diff) : []), [diff]);
  const target = useMemo(
    () => (range ? selectionTargetFromInfos(lines, infos, range.start, range.end) : null),
    [range, lines, infos]
  );

  const clearSelection = () => {
    setRange(null);
    setInstruction("");
  };

  const submit = () => {
    const text = instruction.trim();
    if (!text || !target) return;
    onFeedback?.(buildReviewTask(text, target, t));
    clearSelection();
  };

  return (
    <aside className="zone zone-review" data-testid="review-pane" data-running={running ? "1" : "0"}>
      <div className="review-head">
        <span
          className={`review-dot${running ? " live" : ""}`}
          aria-hidden
        />
        <span className="side-title">{t("review.title")}</span>
        <span className="review-status" data-testid="review-status">
          {statusLabel}
        </span>
        <button
          type="button"
          className="pe-collapse"
          data-testid="review-close"
          title={t("review.close")}
          aria-label={t("review.close")}
          onClick={onClose}
        >
          ✕
        </button>
      </div>
      <div className="review-body">
        {target && (
          <div className="review-feedback" data-testid="review-feedback">
            <div className="review-fb-head">
              <span className="review-fb-target" data-testid="review-fb-target">
                {target.startLine !== undefined
                  ? t("review.fb_target", {
                      path: target.path,
                      start: target.startLine,
                      end: target.endLine ?? target.startLine,
                    })
                  : t("review.fb_target_file", { path: target.path })}
              </span>
              <button
                type="button"
                className="pe-collapse"
                data-testid="review-fb-clear"
                title={t("review.fb_clear")}
                aria-label={t("review.fb_clear")}
                onClick={clearSelection}
              >
                ✕
              </button>
            </div>
            <textarea
              rows={2}
              value={instruction}
              placeholder={t("review.fb_placeholder")}
              aria-label={t("review.fb_title")}
              data-testid="review-fb-input"
              onChange={(e) => setInstruction(e.target.value)}
              onKeyDown={(e) => {
                if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                  e.preventDefault();
                  submit();
                }
              }}
            />
            <div className="review-fb-actions">
              <span className="review-fb-hint">
                {running ? t("review.fb_hint_queued") : t("review.fb_hint_now")}
              </span>
              <button
                type="button"
                className="primary"
                data-testid="review-fb-send"
                disabled={!instruction.trim()}
                onClick={submit}
              >
                {t("review.fb_send")}
              </button>
            </div>
          </div>
        )}
        <DiffPanel
          diff={diff}
          emptyText={running ? t("review.empty_running") : t("review.empty")}
          selectable={Boolean(onFeedback)}
          selection={range}
          onSelectionChange={setRange}
        />
      </div>
    </aside>
  );
}
