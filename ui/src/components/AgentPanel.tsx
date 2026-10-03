// 代理会话面板（设计方案 §7.2 右区）：会话流 / 状态色 / 审批卡 / 证据卡。
// §8.6 人机共编：dirty_conflict 事件 → 三栏合并预览；补丁 → 行级 AI 角标。
import { useEffect, useRef, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { STATE_COLORS, type AgentStateName } from "../lib/stateColors";
import { ApprovalCard, pendingApprovalFromEvents } from "./ApprovalCard";
import { diffFromPatchEvent } from "./DiffPanel";
import { aiLinesFromDiff } from "../lib/aiLines";

export interface DirtyConflictView {
  path: string;
  base: string;
  ours: string;
  theirs: string;
}

interface Props {
  api: TenonApi;
  t: Translate;
  sessionId: string | null;
  onStateChange?: (s: AgentStateName) => void;
  onLatestDiff?: (diff: string | null) => void;
  onDirtyConflict?: (c: DirtyConflictView | null) => void;
  onPatchLines?: (path: string, lines: number[]) => void;
}

interface EventItem {
  id: number;
  seq: number;
  type: string;
  payload: Record<string, unknown>;
}

export function AgentPanel({
  api,
  t,
  sessionId,
  onStateChange,
  onLatestDiff,
  onDirtyConflict,
  onPatchLines,
}: Props) {
  const [events, setEvents] = useState<EventItem[]>([]);
  const [status, setStatus] = useState<AgentStateName>("idle");
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [latestDiff, setLatestDiff] = useState<string | null>(null);
  const feedRef = useRef<HTMLDivElement>(null);
  const patchLinesCb = useRef<Props["onPatchLines"]>(undefined);

  useEffect(() => {
    patchLinesCb.current = onPatchLines;
  }, [onPatchLines]);

  // 事件流轮询（M0：/trace 增量拉取；M1 切 WS 推流）
  useEffect(() => {
    if (!sessionId) return;
    let alive = true;
    let after = 0;
    const tick = async () => {
      try {
        const r = await api.trace(sessionId, after);
        if (!alive) return;
        if (r.events.length > 0) {
          after = r.events[r.events.length - 1].seq;
          setEvents((prev) => [...prev, ...r.events]);
          for (const ev of r.events) {
            if (ev.type === "patch_applied") {
              const d = diffFromPatchEvent(ev.payload);
              if (d) {
                setLatestDiff(d);
                onLatestDiff?.(d);
                const changed = (
                  ev.payload as { output?: { changed_files?: string[] } }
                ).output?.changed_files;
                const path = changed?.[0] ?? "";
                if (path) patchLinesCb.current?.(path, aiLinesFromDiff(d));
              }
            }
            if (
              ev.type === "diagnostics" &&
              (ev.payload as { dirty_conflict?: boolean }).dirty_conflict
            ) {
              onDirtyConflict?.({
                path: String(ev.payload.path ?? ""),
                base: String(ev.payload.base ?? ""),
                ours: String(ev.payload.ours ?? ""),
                theirs: String(ev.payload.theirs ?? ""),
              });
            }
          }
        }
        const s = await api.getSession(sessionId);
        if (!alive) return;
        const name = s.status as AgentStateName;
        setStatus(name);
        onStateChange?.(name);
      } catch {
        // 断线重试
      }
    };
    const timer = setInterval(tick, 500);
    tick();
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [api, sessionId, onStateChange, onLatestDiff, onDirtyConflict]);

  async function send() {
    if (!sessionId || !input.trim()) return;
    setBusy(true);
    try {
      await api.sendMessage(sessionId, input.trim());
      setInput("");
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    if (!sessionId) return;
    await api.control(sessionId, "stop");
  }

  const approval = pendingApprovalFromEvents(events);

  return (
    <div className="agent-panel" data-testid="agent-panel">
      <div className="agent-head">
        <span
          className="state-dot"
          style={{ background: STATE_COLORS[status] ?? "#888" }}
          data-testid="state-dot"
          aria-label={status}
        />
        <span className="state-label">{t(`state.${status}`)}</span>
        <button className="agent-stop" onClick={stop} disabled={!sessionId}>
          {t("message.stop")}
        </button>
      </div>

      <div className="agent-feed" ref={feedRef} data-testid="agent-feed">
        {events.map((e) => (
          <EventCard key={e.id} ev={e} t={t} />
        ))}
        {approval && (
          <ApprovalCard
            api={api}
            t={t}
            approval={approval}
            onDecided={() =>
              setEvents((prev) => [
                ...prev,
                {
                  id: Date.now(),
                  seq: Number.MAX_SAFE_INTEGER,
                  type: "approval_decision_local",
                  payload: {},
                },
              ])
            }
          />
        )}
      </div>

      <div className="agent-input">
        <textarea
          value={input}
          placeholder={t("message.placeholder")}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault();
              send();
            }
            if (e.key === "Escape") {
              (e.target as HTMLTextAreaElement).blur();
            }
          }}
          data-testid="task-input"
        />
        <button onClick={send} disabled={busy || !sessionId} data-testid="send">
          {t("message.send")}
        </button>
      </div>
    </div>
  );
}

function EventCard({ ev, t }: { ev: EventItem; t: Translate }) {
  switch (ev.type) {
    case "user_input":
      return <div className="ev ev-user">{String(ev.payload.text ?? "")}</div>;
    case "decision": {
      const intent = String(ev.payload.intent ?? "");
      if (ev.payload.first_edit === true) {
        return <div className="ev ev-info">⏳ {t("state.executing")}…</div>;
      }
      return intent ? <div className="ev ev-decision">{intent}</div> : null;
    }
    case "patch_applied":
      return <div className="ev ev-patch">✎ {String(ev.payload.tool ?? "")}</div>;
    case "command_run":
      return <div className="ev ev-cmd">▶ {String(ev.payload.tool ?? "")}</div>;
    case "diagnostics":
      return (
        <div className="ev ev-verify">
          <strong>{t("evidence.title")}</strong>
          <pre>{String((ev.payload as { verification?: string }).verification ?? "")}</pre>
        </div>
      );
    case "approval_request":
      return null; // 由 ApprovalCard 呈现
    case "error":
      return (
        <div className="ev ev-error">
          ⚠ {String((ev.payload as { error?: string }).error ?? "error")}
        </div>
      );
    default:
      return null;
  }
}
