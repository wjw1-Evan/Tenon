// 全局活动视图（v2.12，design-v2.md §6「命令面板 + 侧栏入口的跨项目任务总览」）：
// 按需模态浮层——跨项目任务聚合总览（rail activity 钮 + 命令面板双入口）。
// v1.98 克制不变式保持：非常驻（按需浮层）、无系统推送、attention 以徽标为准；
// 数据复用 App 既有 GET /projects 轮询（零新端点，§3「跨项目只做全局任务聚合」）。
// 任务行语义复用 v1.114：状态色点 + 清洗标题 + 相对时间 + 子任务 n/m 徽标；
// 行点击 = activateProject + selectSession 既有链激活跳转并关闭浮层。
import { useEffect, useMemo, useRef } from "react";
import type { ProjectSummary } from "../lib/api";
import { RUNNING_STATES, STATE_COLORS, type AgentStateName } from "../lib/stateColors";
import { useResolvedLocale } from "../lib/i18n";
import type { Translate } from "../lib/i18n";

/** 显示层标题清洗（v1.101 同款，ProjectExplorer 私有实现的轻量副本）。 */
const TITLE_PROMPT_PREFIX_RE =
  /^the user(?:'|’)?s?\s+(?:message|says?|wants?|asks?|requests?)[^:]{0,32}:\s*/i;

function cleanDisplayTitle(raw: string): string {
  let s = raw.trim();
  let prev = "";
  while (s !== prev) {
    prev = s;
    s = s.replace(TITLE_PROMPT_PREFIX_RE, "").trim();
  }
  return s.replace(/^["'“”「『«]+|["'”』»]+$/g, "").trim();
}

function formatUpdatedAt(value: string | null, t: Translate, locale: string): string {
  if (!value) return t("projects.updated_never");
  const time = Date.parse(value);
  if (Number.isNaN(time)) return t("projects.updated_never");
  const elapsed = Math.max(0, Date.now() - time);
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return t("relative.now");
  if (minutes < 60) return t("relative.minutes_ago", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("relative.hours_ago", { count: hours });
  const days = Math.floor(hours / 24);
  if (days < 30) return t("relative.days_ago", { count: days });
  return new Date(time).toLocaleDateString(locale);
}

interface Props {
  open: boolean;
  onClose: () => void;
  projects: ProjectSummary[];
  t: Translate;
  /** 行点击：激活该项目的该会话（App 侧 activateProject + selectSession 链）。 */
  onOpenSession: (projectId: string, sessionId: string) => void;
}

export function GlobalActivityDialog({ open, onClose, projects, t, onOpenSession }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const locale = useResolvedLocale();

  useEffect(() => {
    if (!open) return;
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  // 分组：有任务（含标题的会话）才渲染组；含运行中会话的项目置顶，组内按更新时间倒序
  const groups = useMemo(() => {
    return projects
      .map((project) => {
        const sessions = [...project.sessions]
          .filter((s) => cleanDisplayTitle(s.title ?? "") || s.model)
          .sort((a, b) => Date.parse(b.updated_at ?? "") - Date.parse(a.updated_at ?? ""));
        return { project, sessions };
      })
      .filter((g) => g.sessions.length > 0)
      .sort((a, b) => {
        const aRunning = a.sessions.some((s) => RUNNING_STATES.has(s.status as AgentStateName));
        const bRunning = b.sessions.some((s) => RUNNING_STATES.has(s.status as AgentStateName));
        if (aRunning !== bRunning) return aRunning ? -1 : 1;
        // 同为（非）运行组：组内最新会话时间倒序
        const aLatest = Date.parse(a.sessions[0]?.updated_at ?? "");
        const bLatest = Date.parse(b.sessions[0]?.updated_at ?? "");
        return bLatest - aLatest;
      });
  }, [projects]);

  if (!open) return null;
  const runningCount = projects.reduce(
    (sum, p) =>
      sum +
      p.sessions.filter((s) => RUNNING_STATES.has(s.status as AgentStateName)).length,
    0
  );

  return (
    <div className="palette-overlay" onClick={onClose} data-testid="global-activity">
      <div
        className="activity-pane"
        role="dialog"
        aria-modal="true"
        aria-label={t("activity.title")}
        tabIndex={-1}
        ref={ref}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            onClose();
          }
        }}
      >
        <div className="activity-head">
          <strong>{t("activity.title")}</strong>
          {runningCount > 0 && (
            <span className="activity-running" data-testid="activity-running">
              {t("activity.running_count", { count: runningCount })}
            </span>
          )}
        </div>
        <div className="activity-body">
          {groups.length === 0 && <div className="activity-empty">{t("activity.empty")}</div>}
          {groups.map(({ project, sessions }) => (
            <section key={project.id} className="activity-group" data-testid="activity-group">
              <div className="activity-group-head">{project.display_name}</div>
              <ul>
                {sessions.map((session) => {
                  const status = session.status as AgentStateName;
                  const running = RUNNING_STATES.has(status);
                  const subtasks = session.subtasks;
                  return (
                    <li key={session.id}>
                      <button
                        type="button"
                        className={`activity-row${running ? " running" : ""}`}
                        data-testid="activity-row"
                        onClick={() => onOpenSession(project.id, session.id)}
                      >
                        <span
                          className={`activity-dot${running ? " live" : ""}`}
                          style={{ background: STATE_COLORS[status] ?? "#888" }}
                          aria-hidden
                        />
                        <span className="activity-title">
                          {cleanDisplayTitle(session.title ?? "") || session.model}
                        </span>
                        {subtasks && subtasks.done < subtasks.total && (
                          <span className="activity-subtasks">
                            {subtasks.done}/{subtasks.total}
                          </span>
                        )}
                        <span className="activity-time">
                          {formatUpdatedAt(session.updated_at, t, locale)}
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
