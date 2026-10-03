// 审批卡片（设计方案 §7.3）：级别、动作详情、允许一次 / 本会话 / 拒绝。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

export interface ApprovalInfo {
  approvalId: string;
  tool: string;
  level: "a" | "b" | "c" | "d";
  summary: string;
}

interface Props {
  api: TenonApi;
  t: Translate;
  approval: ApprovalInfo;
  onDecided?: (decision: "once" | "session" | "deny") => void;
}

const LEVEL_LABEL: Record<string, string> = {
  a: "A · read-only",
  b: "B · sandbox write",
  c: "C · network",
  d: "D · irreversible",
};

export function ApprovalCard({ api, t, approval, onDecided }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function decide(decision: "once" | "session" | "deny") {
    setBusy(true);
    setError(null);
    try {
      await api.decideApproval(approval.approvalId, decision);
      onDecided?.(decision);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="approval-card" data-level={approval.level} data-testid="approval-card">
      <div className="approval-head">
        <strong>{t("approval.title")}</strong>
        <span className="approval-level">
          {t("approval.level")}: {LEVEL_LABEL[approval.level] ?? approval.level}
        </span>
      </div>
      <div className="approval-body">
        <code className="approval-tool">{approval.tool}</code>
        <span className="approval-summary">{approval.summary}</span>
      </div>
      {error && <div role="alert">{error}</div>}
      <div className="approval-actions">
        <button disabled={busy} onClick={() => decide("once")} data-testid="approve-once">
          {t("approval.allow_once")}
        </button>
        <button disabled={busy} onClick={() => decide("session")} data-testid="approve-session">
          {t("approval.allow_session")}
        </button>
        <button disabled={busy} onClick={() => decide("deny")} data-testid="approve-deny">
          {t("approval.deny")}
        </button>
      </div>
    </div>
  );
}

/** 从事件流提取待审批卡（§15 approval_request）。 */
export function pendingApprovalFromEvents(
  events: Array<{ type: string; payload: Record<string, unknown> }>
): ApprovalInfo | null {
  const last = [...events].reverse().find((e) => e.type === "approval_request");
  if (!last) return null;
  const p = last.payload;
  return {
    approvalId: String(p.approval_id ?? ""),
    tool: String(p.tool ?? ""),
    level: (p.level as ApprovalInfo["level"]) ?? "c",
    summary: String(p.summary ?? ""),
  };
}
