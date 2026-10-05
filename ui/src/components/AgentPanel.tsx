// 代理会话面板（设计方案 §7.2 右区）：会话过程流 / 状态色 / 步骤卡；v1.89 无审批。
// v1.109 会话过程流重构为 Codex 形态（§7.5「会话过程流」）：事件流按 user_input 切分回合，
// 工具调用渲染为折叠步骤卡（动词摘要 + 目标摘要，展开看 args 与输出），助手正文走
// 受限 markdown 渲染器；运行态活跃回合底部 spinner 行承接「正在做什么」与流式草稿。
// §8.6 人机共编：dirty_conflict 事件 → 三栏合并预览；补丁 → 行级 AI 角标。
// v1.51：模型选择入口内嵌任务输入框底行（参考 ZCode 客户端输入区）。
import { useEffect, useMemo, useRef, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { RUNNING_STATES, STATE_COLORS, type AgentStateName } from "../lib/stateColors";
import { renderMarkdownLite } from "../lib/markdownLite";
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
  /** 新任务草稿态（v1.116 §7.2）：sessionId 为空时输入仍可用，首发经 onDraftSend 建会话。 */
  draft?: boolean;
  /** 草稿首发回调：App 落库建会话（按草稿意图附 worktree）后发送并回填激活会话。 */
  onDraftSend?: (text: string) => Promise<void>;
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

/** 回合（turn）：一个 user_input 及其派生的全部事件；id=0 为引导组（user_input 之前的尾部事件）。 */
interface Turn {
  id: number;
  task: string | null;
  items: EventItem[];
}

/** §9.2 内置工具协议集合：已知工具走动作短语，外部 / 未知工具原样显示名。 */
const KNOWN_TOOLS = new Set([
  "read_file",
  "list_dir",
  "grep",
  "git_read",
  "lsp_query",
  "apply_patch",
  "run_tests",
  "run_build",
  "install_deps",
  "http_fetch",
  "git_commit",
  "git_push",
  "create_pr",
]);

/** 步骤卡目标摘要依次提取的 args 关键字段。 */
const TARGET_ARG_KEYS = [
  "command",
  "cmd",
  "path",
  "file",
  "file_path",
  "dir",
  "url",
  "query",
  "pattern",
  "symbol",
  "name",
  "message",
];

/** 展开区输出 pre 的字符上限（完整事件表由底部「轨迹」tab 承载）。 */
const OUTPUT_LIMIT = 4000;
const ARGS_LIMIT = 2000;

function toolLabel(tool: string, t: Translate): string {
  return KNOWN_TOOLS.has(tool) ? t(`tool.${tool}`) : tool;
}

function stepSummary(ev: EventItem): { tool: string; target: string; failed: boolean } {
  const tool = String(ev.payload.tool ?? "");
  const args = ev.payload.args as Record<string, unknown> | undefined;
  const output = ev.payload.output as { changed_files?: unknown[]; ok?: boolean } | undefined;
  const changed = Array.isArray(output?.changed_files) ? output!.changed_files! : [];
  let target = "";
  if (changed.length > 0) {
    const first = String(changed[0]);
    target = changed.length > 1 ? `${first} +${changed.length - 1}` : first;
  } else if (args) {
    for (const key of TARGET_ARG_KEYS) {
      const v = args[key];
      if (typeof v === "string" && v.trim()) {
        target = v.trim();
        break;
      }
    }
  }
  return { tool, target, failed: output?.ok === false };
}

function truncate(text: string, limit: number): { text: string; more: boolean } {
  if (text.length <= limit) return { text, more: false };
  return { text: `${text.slice(0, limit)}…`, more: true };
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

export function AgentPanel({
  api,
  t,
  sessionId,
  draft,
  onDraftSend,
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
  // 文件拖入对话框（v1.110）：dragover 高亮 + 落下插入 @path 引用。
  const [inputDrop, setInputDrop] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
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
  }, [events.length, streamText, status]);

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
    const text = input.trim();
    if (!text) return;
    // 草稿任务首发（v1.116）：此刻才建会话（App 落库后回填激活会话），随后的
    // 发送经既有路径；失败保留输入可重试，错误经 App 层呈现。
    if (!sessionId) {
      if (draft && onDraftSend) {
        setBusy(true);
        try {
          await onDraftSend(text);
          setInput("");
        } finally {
          setBusy(false);
        }
      }
      return;
    }
    setBusy(true);
    try {
      await api.sendMessage(sessionId, text);
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

  // v1.109：回合分组——user_input 开新回合，其余事件进当前回合；model_delta 由 streamText 聚合不入列。
  const turns = useMemo(() => {
    const list: Turn[] = [];
    let cur: Turn | null = null;
    for (const ev of events) {
      if (ev.type === "user_input") {
        cur = { id: ev.id, task: String(ev.payload.text ?? ""), items: [] };
        list.push(cur);
        continue;
      }
      if (ev.type === "model_delta") continue;
      if (!cur) {
        cur = { id: 0, task: null, items: [] };
        list.push(cur);
      }
      cur.items.push(ev);
    }
    return list;
  }, [events]);

  // 运行态「正在做什么」：取最后一个 decision 的意图；first_edit 无意图则显示执行中态。
  const runningLabel = useMemo(() => {
    for (let i = events.length - 1; i >= 0; i -= 1) {
      const ev = events[i];
      if (ev.type === "decision") {
        if (ev.payload.first_edit === true) return t("state.executing");
        const intent = decisionDisplayText(String(ev.payload.intent ?? ""));
        return intent || t("thread.running");
      }
    }
    return t("thread.running");
  }, [events, t]);

  const empty = events.length === 0 && !streamText;

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
        {empty && (
          <div className="agent-empty" data-testid="agent-empty">
            {sessionId || draft ? t("thread.empty") : t("thread.no_session")}
          </div>
        )}
        {turns.map((turn, idx) => {
          const active = idx === turns.length - 1;
          return (
            <section className="turn" key={turn.id} data-testid="turn">
              {turn.task !== null && <div className="turn-task">{turn.task}</div>}
              <div className="turn-body">
                {turn.items.map((ev) => (
                  <EventNode key={ev.id} ev={ev} t={t} />
                ))}
                {active && streamText && (
                  <div
                    className="turn-answer"
                    data-testid="model-stream"
                    dangerouslySetInnerHTML={{ __html: renderMarkdownLite(streamText) }}
                  />
                )}
                {active && running && !streamText && (
                  <div className="turn-running" data-testid="turn-running">
                    <span className="turn-spinner" aria-hidden="true" />
                    {runningLabel}
                  </div>
                )}
              </div>
            </section>
          );
        })}
      </div>

      <div
        className={inputDrop ? "agent-input drop-target" : "agent-input"}
        data-testid="task-input-box"
        onDragOver={(e) => {
          // 文件拖入对话框（v1.110）：接受文件树行的私有 MIME 拖拽。
          if (e.dataTransfer.types.includes("application/x-tenon-path")) {
            e.preventDefault();
            setInputDrop(true);
          }
        }}
        onDragLeave={(e) => {
          if (e.currentTarget === e.target) setInputDrop(false);
        }}
        onDrop={(e) => {
          const path = e.dataTransfer.getData("application/x-tenon-path");
          setInputDrop(false);
          if (!path) return;
          e.preventDefault();
          setInput((prev) => (prev.trimEnd() ? `${prev.trimEnd()} @${path} ` : `@${path} `));
          inputRef.current?.focus();
        }}
      >
        <textarea
          ref={inputRef}
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
            disabled={(!sessionId && !(draft && onDraftSend)) || stopRequested || (!running && !paused && busy)}
            data-testid={paused ? "resume" : running ? "stop" : "send"}
          >
            {paused ? t("message.resume") : running ? t("message.stop_short") : t("message.send")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 事件 → 会话流节点；只渲染面向用户的子集，完整事件表由底部「轨迹」tab 承载。 */
function EventNode({ ev, t }: { ev: EventItem; t: Translate }) {
  switch (ev.type) {
    case "decision":
      // first_edit 由活跃回合底部 spinner 行承接；普通决策为回合 markdown 正文段。
      if (ev.payload.first_edit === true) return null;
      return (
        <div
          className="turn-answer"
          dangerouslySetInnerHTML={{
            __html: renderMarkdownLite(decisionDisplayText(String(ev.payload.intent ?? ""))),
          }}
        />
      );
    case "patch_applied":
    case "command_run":
      return <StepCard ev={ev} t={t} risk={false} />;
    case "direct_action":
      return <StepCard ev={ev} t={t} risk={true} />;
    case "diagnostics":
      if ((ev.payload as { dirty_conflict?: boolean }).dirty_conflict) return null;
      return (
        <div className="turn-verify">
          <strong>{t("evidence.title")}</strong>
          <pre>{String((ev.payload as { verification?: string }).verification ?? "")}</pre>
        </div>
      );
    case "error": {
      const denied = (ev.payload as { denied?: string }).denied;
      if (denied) {
        return (
          <div className="turn-error">
            ⚠ {t("thread.denied")} {denied}
            {ev.payload.reason !== undefined ? ` · ${String(ev.payload.reason)}` : ""}
          </div>
        );
      }
      return (
        <div className="turn-error">
          ⚠ {String((ev.payload as { error?: string }).error ?? t("agent.error_fallback"))}
        </div>
      );
    }
    case "rollback":
      return <div className="turn-note">↩ {t("thread.rollback")}</div>;
    case "unrollback":
      return <div className="turn-note">↪ {t("thread.unrollback")}</div>;
    case "model_fallback":
      return (
        <div className="turn-note turn-note-warn">
          ⚠ {t("thread.model_fallback")}
          {ev.payload.reason !== undefined ? ` · ${String(ev.payload.reason)}` : ""}
        </div>
      );
    case "compaction":
      return <div className="turn-note">⧉ {t("thread.compaction")}</div>;
    case "memory_saved":
      return <div className="turn-note">✦ {t("thread.memory_saved")}</div>;
    default:
      // sensing / decider_call / checkpoint / session_title / 未知类型不渲染
      return null;
  }
}

/** 工具步骤卡（v1.109）：折叠标题行 = 状态图标 + 动词摘要 + 目标摘要；展开看 args 与输出；失败红显默认展开。 */
function StepCard({ ev, t, risk }: { ev: EventItem; t: Translate; risk: boolean }) {
  const [userOpen, setUserOpen] = useState(false);
  const { tool, target, failed } = stepSummary(ev);
  const expanded = userOpen || failed;
  const args = ev.payload.args;
  const output = ev.payload.output as { content?: string } | undefined;
  const argsText = args === undefined ? null : truncate(JSON.stringify(args, null, 2), ARGS_LIMIT);
  const outText = output?.content ? truncate(output.content, OUTPUT_LIMIT) : null;
  const level = risk ? String(ev.payload.level ?? "") : "";

  return (
    <div
      className={`turn-step${failed ? " turn-step-fail" : ""}${risk ? " turn-step-risk" : ""}`}
      data-testid="turn-step"
    >
      <button
        type="button"
        className="turn-step-head"
        aria-expanded={expanded}
        onClick={() => setUserOpen((v) => !v)}
      >
        <span className="turn-step-icon" aria-hidden="true">
          {failed ? "✗" : risk ? "⚡" : "✓"}
        </span>
        {level && <span className="lvl-badge">{level}</span>}
        <span className="turn-step-label">
          {toolLabel(tool, t)}
          {target ? ` · ${target}` : ""}
        </span>
        {failed && <span className="step-badge">{t("thread.failed")}</span>}
      </button>
      {expanded && (
        <div className="turn-step-body">
          {argsText && (
            <>
              <div className="turn-step-cap">{t("thread.args")}</div>
              <pre>{argsText.text}</pre>
              {argsText.more && <div className="turn-step-more">{t("thread.output_truncated")}</div>}
            </>
          )}
          {outText && (
            <>
              <div className="turn-step-cap">{t("thread.output")}</div>
              <pre>{outText.text}</pre>
              {outText.more && <div className="turn-step-more">{t("thread.output_truncated")}</div>}
            </>
          )}
        </div>
      )}
    </div>
  );
}
