// 顶栏安全姿态指示（v2.17，design-v2.md §4.6）：ExecMode×Approval 常驻紧凑
// 徽标——盾形 + 双档缩写，read_only 绿 / full_access 红 / 其余琥珀；
// active 会话随 gate 快照实时（无会话不渲染）；点击直达设置 Permissions 节。
import type { Translate } from "../lib/i18n";

const EXEC_SHORT: Record<string, string> = {
  read_only: "RO",
  workspace_write: "WW",
  full_access: "FA",
};

const APPROVAL_SHORT: Record<string, string> = {
  never: "N",
  on_irreversible: "OI",
  always: "A",
};

export function SecurityBadge({
  t,
  gate,
  onClick,
}: {
  t: Translate;
  gate: { exec_mode: string; approval: string };
  /** 点击 = 打开设置 Permissions 节（App 持有设置面板状态）。 */
  onClick: () => void;
}) {
  const exec = gate.exec_mode;
  const tone =
    exec === "read_only" ? "safe" : exec === "full_access" ? "risky" : "caution";
  const execLabel =
    exec === "read_only"
      ? t("gates.exec_read_only")
      : exec === "full_access"
        ? t("gates.exec_full_access")
        : t("gates.exec_workspace_write");
  const approvalLabel =
    gate.approval === "never"
      ? t("gates.approval_never")
      : gate.approval === "always"
        ? t("gates.approval_always")
        : t("gates.approval_on_irreversible");
  return (
    <button
      type="button"
      className={`security-badge tone-${tone}`}
      data-testid="security-badge"
      data-exec-mode={exec}
      data-approval={gate.approval}
      title={`${t("topbar.security")}: ${execLabel} × ${approvalLabel}`}
      aria-label={`${t("topbar.security")}: ${execLabel} × ${approvalLabel}`}
      onClick={onClick}
    >
      <svg
        width={12}
        height={12}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
      </svg>
      <span className="security-badge-mode">
        {EXEC_SHORT[exec] ?? exec} · {APPROVAL_SHORT[gate.approval] ?? gate.approval}
      </span>
    </button>
  );
}
