// 代理会话面板（设计方案 §7.2 右区）：会话流 / 状态色 / 证据卡；v1.89 无审批。
// §8.6 人机共编：dirty_conflict 事件 → 三栏合并预览；补丁 → 行级 AI 角标。
// v1.51：模型选择入口内嵌任务输入框底行（参考 ZCode 客户端输入区）。
import { useEffect, useRef, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { RUNNING_STATES, STATE_COLORS, type AgentStateName } from "../lib/stateColors";
import { diffFromPatchEvent } from "./DiffPanel";
import { aiLinesFromDiff } from "../lib/aiLines";
import { ModelRoutingPanel } from "./ModelRoutingPanel";
import { markWorkspaceInputReady } from "../lib/performance";

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
  /** 跟随模式（§8.5）：代理写入时编辑器自动滚动到改动处；App 持有状态，此处仅展示开关。 */
  followMode?: boolean;
  onToggleFollow?: () => void;
  /** 外部一键入口（诊断修复）注入任务；token 变化即发送。 */
  injectedTask?: { token: number; text: string };
  /** 会话级模型热切换回调（§11：上下文随迁提示由 App 呈现）。 */
  onModelSwitched?: (model: string) => void;
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
  followMode,
  onToggleFollow,
  injectedTask,
  onModelSwitched,
}: Props) {
  const [events, setEvents] = useState<EventItem[]>([]);
  const [streamText, setStreamText] = useState("");
  const [status, setStatus] = useState<AgentStateName>("idle");
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [stopRequested, setStopRequested] = useState(false);
  const [latestDiff, setLatestDiff] = useState<string | null>(null);
  const feedRef = useRef<HTMLDivElement>(null);
  const patchLinesCb = useRef<Props["onPatchLines"]>(undefined);

  useEffect(() => {
    patchLinesCb.current = onPatchLines;
  }, [onPatchLines]);

  // §8.7 冷启动终点：任务输入框已挂载并完成两帧渲染，用户可开始键入。
  useEffect(() => {
    markWorkspaceInputReady();
  }, []);

  // 会话切换即清空本地事件流：轮询按 after=0 重新拉取，若不清空，
  // 上一会话的事件残留会导致新会话线程显示旧会话消息（实测缺陷）。
  useEffect(() => {
    setEvents([]);
    setStreamText("");
    setStatus("idle");
    setLatestDiff(null);
    setStopRequested(false);
  }, [sessionId]);

  // 流式输出保持最新增量可见；用户向上回看时不强制拉底。
  useEffect(() => {
    const feed = feedRef.current;
    if (feed && feed.scrollHeight - feed.scrollTop - feed.clientHeight < 160) {
      feed.scrollTop = feed.scrollHeight;
    }
  }, [events.length, streamText]);

  // §8.1 / §8.5 诊断「AI 修复」：直接发起当前项目会话任务（T4 入口）。
  useEffect(() => {
    const text = injectedTask?.text.trim();
    if (!text || !sessionId) return;
    let alive = true;
    void api
      .sendMessage(sessionId, text)
      .catch(() => {})
      .finally(() => {
        if (alive) setInput("");
      });
    return () => {
      alive = false;
    };
  }, [api, sessionId, injectedTask]);

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
            if (ev.type === "user_input") setStreamText("");
            if (ev.type === "model_delta") {
              setStreamText((prev) => prev + String(ev.payload.text ?? ""));
            }
            if (ev.type === "decision") setStreamText("");
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

  // v1.59：运行态下发送按钮变「停止」——协作暂停在下一工具调用检查点生效。
  // stop 受理即禁用，防状态轮询间隙连点向控制队列残留多条 Stop（回到空闲态解锁）。
  useEffect(() => {
    if (!RUNNING_STATES.has(status)) setStopRequested(false);
  }, [status]);

  async function stop() {
    if (!sessionId) return;
    setStopRequested(true);
    await api.control(sessionId, "stop");
  }

  // v1.92：暂停只进不出的修复——paused 时发送钮承担恢复。
  async function resume() {
    if (!sessionId) return;
    await api.control(sessionId, "resume");
  }

  const running = RUNNING_STATES.has(status);
  const paused = status === "paused";

  return (
    <div className="agent-panel" data-testid="agent-panel">
      <div className="agent-head">
        <span
          className="state-dot"
          style={{ background: STATE_COLORS[status] ?? "#888" }}
          data-state={status}
          data-testid="state-dot"
          aria-label={status}
        />
        <span className="state-label">{t(`state.${status}`)}</span>
        <button
          type="button"
          className={followMode ? "agent-follow on" : "agent-follow"}
          aria-pressed={followMode ?? false}
          title={t("follow.toggle")}
          aria-label={t("follow.toggle")}
          data-testid="follow-toggle"
          onClick={onToggleFollow}
        >
          {t("follow.toggle")}
        </button>
      </div>

      <div className="agent-feed" ref={feedRef} data-testid="agent-feed">
        {events.length === 0 && !streamText && (
          <div className="agent-empty" data-testid="agent-empty">
            {sessionId ? t("thread.empty") : t("thread.no_session")}
          </div>
        )}
        {events.map((e) => (
          <EventCard key={e.id} ev={e} t={t} />
        ))}
        {streamText && (
          <div className="ev ev-decision" data-testid="model-stream">
            {streamText}
          </div>
        )}
      </div>

      <div className="agent-input" data-testid="task-input-box">
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
        <div className="agent-input-foot">
          <ModelRoutingPanel
            api={api}
            sessionId={sessionId}
            t={t}
            onSwitched={onModelSwitched}
          />
          <button
            className={running ? "agent-send agent-send-stop" : "agent-send"}
            onClick={paused ? resume : running ? stop : send}
            disabled={!sessionId || stopRequested || (!running && !paused && busy)}
            data-testid={paused ? "resume" : running ? "stop" : "send"}
          >
            {paused ? t("message.resume") : running ? t("message.stop_short") : t("message.send")}
          </button>
        </div>
      </div>
    </div>
  );
}

function EventCard({ ev, t }: { ev: EventItem; t: Translate }) {
  switch (ev.type) {
    case "user_input":
      return <div className="ev ev-user">{String(ev.payload.text ?? "")}</div>;
    case "model_delta":
      // 连续增量由上方单个 stream 卡承接，避免 token / 批次渲染成事件瀑布。
      return null;
    case "decision": {
      const intent = decisionDisplayText(String(ev.payload.intent ?? ""));
      if (ev.payload.first_edit === true) {
        return <div className="ev ev-info">⏳ {t("state.executing")}…</div>;
      }
      return intent ? <div className="ev ev-decision">{intent}</div> : null;
    }
    case "patch_applied":
      return <div className="ev ev-patch">✎ {String(ev.payload.tool ?? "")}</div>;
    case "direct_action":
      return (
        <div className="ev ev-cmd">
          ⚡ {String(ev.payload.tool ?? "")} · {String(ev.payload.level ?? "")}
        </div>
      );
    case "command_run":
      return <div className="ev ev-cmd">▶ {String(ev.payload.tool ?? "")}</div>;
    case "diagnostics":
      return (
        <div className="ev ev-verify">
          <strong>{t("evidence.title")}</strong>
          <pre>{String((ev.payload as { verification?: string }).verification ?? "")}</pre>
        </div>
      );
    case "error":
      return (
        <div className="ev ev-error">
          ⚠ {String((ev.payload as { error?: string }).error ?? t("agent.error_fallback"))}
        </div>
      );
    default:
      return null;
  }
}

/** 决策 / 收尾回答可能是模型输出的 JSON（intent/answer 结构）——解出人类可读部分呈现。 */
function decisionDisplayText(raw: string): string {
  const text = raw.trim();
  if (!text.startsWith("{") || !text.endsWith("}")) return text;
  try {
    const parsed = JSON.parse(text) as { answer?: unknown; intent?: unknown };
    for (const key of ["answer", "intent"] as const) {
      const value = parsed[key];
      if (typeof value === "string" && value.trim()) return value.trim();
    }
  } catch {
    // 非 JSON 原样呈现
  }
  return text;
}
