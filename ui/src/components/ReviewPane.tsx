// v2.4 常驻审查面（design-v2.md §6「审查面是第二主区」）：会话运行中自动停靠
// 右栏，实时呈现代理改动 diff 流——「人改与 AI 改实时互见」从口号变默认形态。
// 空闲自动收起；用户手动关闭后不再自动弹出（会话切换重置）。
import { DiffPanel } from "./DiffPanel";
import type { Translate } from "../lib/i18n";

export function ReviewPane({
  t,
  diff,
  running,
  statusLabel,
  onClose,
}: {
  t: Translate;
  diff: string | null;
  running: boolean;
  statusLabel: string;
  onClose: () => void;
}) {
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
        <DiffPanel
          diff={diff}
          emptyText={running ? t("review.empty_running") : t("review.empty")}
        />
      </div>
    </aside>
  );
}
