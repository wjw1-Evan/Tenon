// 代理会话面板（设计方案 §7.2 右区）：会话过程流 / 状态色 / 步骤卡；v1.89 无审批。
// v1.109 会话过程流重构为 Codex 形态（§7.5「会话过程流」）：事件流按 user_input 切分回合，
// 工具调用渲染为折叠步骤卡（动词摘要 + 目标摘要，展开看 args 与输出），助手正文走
// 受限 markdown 渲染器；运行态活跃回合底部 spinner 行承接「正在做什么」与流式草稿。
// §8.6 人机共编：dirty_conflict 事件 → 三栏合并预览；补丁 → 行级 AI 角标。
// v1.51：模型选择入口内嵌任务输入框底行（参考 ZCode 客户端输入区）。
import { useEffect, useMemo, useRef, useState } from "react";
import type { ProjectSummary, QueuedMessage, TenonApi } from "../lib/api";
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
  /** 草稿工作区意图（v1.126）：true = 受管 worktree（上下文条工作区选择映射 draftByProject）。 */
  draftWorktree?: boolean;
  /** 已打开项目清单 + active 项目（v1.126）：输入区上方上下文条——草稿态选择目标项目 / 工作区。 */
  projects?: ProjectSummary[];
  projectId?: string | null;
  /** 草稿切换目标项目（v1.126）：激活目标项目并保持 / 进入其草稿（同侧栏点项目行语义）。 */
  onSwitchDraftProject?: (project: ProjectSummary) => void;
  /** 草稿工作区意图切换（v1.126）：主根 ↔ 受管 worktree。 */
  onChangeDraftWorktree?: (worktree: boolean) => void;
  /** 当前会话是否受管 worktree（v1.126）：会话态上下文条 ⎇ 徽标。 */
  sessionWorktree?: boolean;
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
  /** v1.111 消息级撤销：回合内首个 patch_applied 事件 seq（对应该步写前 checkpoint）。 */
  firstPatchSeq: number | null;
  /** v1.129 模型用量观测：回合内全部 decision 的 usage 聚合（求和；无则缺省）。 */
  usage: TurnUsageTotal;
  /** v1.131 回合模型标注：回合内首个携带 model 的 decision（热切换 / fallback 后各回合如实）。 */
  model: string | null;
}

/** 回合聚合用量：命中率 = cached / input，速度 = output / durationMs（§11 v1.129）。 */
interface TurnUsageTotal {
  input: number;
  output: number;
  cached: number;
  durationMs: number;
}

/** §9.2 内置工具协议集合：已知工具走动作短语，外部 / 未知工具原样显示名。 */
const KNOWN_TOOLS = new Set([
  "read_file",
  "list_dir",
  "grep",
  "git_read",
  "lsp_query",
  "laya_decide",
  "subtasks",
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
  draftWorktree,
  projects,
  projectId,
  onSwitchDraftProject,
  onChangeDraftWorktree,
  sessionWorktree,
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
  // v1.135：撤销截断后整流重载（递增触发轮询 effect 以 after=0 重拉）。
  const [traceEpoch, setTraceEpoch] = useState(0);
  // 草稿输入文本 per-project（v1.126）：多项目并行草稿互不串扰——
  // 发送成功随草稿清除；弃草稿重进「＋ 新任务」时文本恢复（草稿意图 v1.116 同语义）。
  const [draftInputs, setDraftInputs] = useState<Record<string, string>>({});
  // 文件拖入对话框（v1.110）：dragover 高亮 + 落下插入 @path 引用。
  const [inputDrop, setInputDrop] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const [busy, setBusy] = useState(false);
  const [stopRequested, setStopRequested] = useState(false);
  const [latestDiff, setLatestDiff] = useState<string | null>(null);
  // v1.147 发送消息队列（§9.1）：运行态入队的待发消息，随 GET /session/:id 轮询刷新。
  const [queue, setQueue] = useState<QueuedMessage[]>([]);
  const feedRef = useRef<HTMLDivElement>(null);
  const patchLinesCb = useRef<Props["onPatchLines"]>(undefined);

  useEffect(() => {
    patchLinesCb.current = onPatchLines;
  }, [onPatchLines]);

  // §8.7 冷启动终点：任务输入框已挂载并完成两帧渲染，用户可开始键入。
  useEffect(() => {
    markWorkspaceInputReady();
  }, []);

  // 草稿态（v1.116）：无会话但持有待启动意图——输入可用，首发建会话。
  const isDraft = !sessionId && draft === true;
  const draftKey = projectId ?? "";
  // 草稿文本 per-project（v1.126）；会话态沿用单份输入缓冲（现状语义）。
  const inputValue = isDraft ? (draftInputs[draftKey] ?? "") : input;
  const setInputValue = (v: string) => {
    if (isDraft) setDraftInputs((prev) => ({ ...prev, [draftKey]: v }));
    else setInput(v);
  };
  const activeProject = projects?.find((p) => p.id === projectId);
  const showContext = Boolean(projectId && projects && projects.length > 0 && (isDraft || sessionId));

  // 会话切换即清空本地事件流：轮询按 after=0 重新拉取，若不清空，
  // 上一会话的事件残留会导致新会话线程显示旧会话消息（实测缺陷）。
  useEffect(() => {
    setEvents([]);
    setStreamText("");
    setStatus("idle");
    setLatestDiff(null);
    setStopRequested(false);
    setQueue([]);
  }, [sessionId]);

  // 流式输出保持最新增量可见；用户向上回看时不强制拉底。
  useEffect(() => {
    const feed = feedRef.current;
    if (feed && feed.scrollHeight - feed.scrollTop - feed.clientHeight < 160) {
      feed.scrollTop = feed.scrollHeight;
    }
  }, [events.length, streamText, status]);

  // §8.1 / §8.5 诊断「AI 修复」：直接发起当前项目会话任务（T4 入口）。
  // v1.126：主根草稿态改走草稿首发链路（建会话再发），不再静默丢弃；
  // 受管 worktree 草稿不接——诊断属项目主根上下文，注入 worktree 会话会写副本而非用户所见文件。
  useEffect(() => {
    const text = injectedTask?.text.trim();
    if (!text) return;
    if (sessionId) {
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
    }
    if (isDraft && draftWorktree !== true && onDraftSend) {
      void onDraftSend(text).catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
        setQueue(s.queue ?? []);
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
  }, [api, sessionId, onStateChange, onLatestDiff, onDirtyConflict, traceEpoch]);

  async function send() {
    const text = inputValue.trim();
    if (!text) return;
    // 草稿任务首发（v1.116）：此刻才建会话（App 落库后回填激活会话），随后的
    // 发送经既有路径；失败保留输入可重试，错误经 App 层呈现。
    if (!sessionId) {
      if (draft && onDraftSend) {
        setBusy(true);
        try {
          await onDraftSend(text);
          // 草稿文本随草稿清除（v1.126 per-project）。
          setDraftInputs((prev) => {
            if (!(draftKey in prev)) return prev;
            const next = { ...prev };
            delete next[draftKey];
            return next;
          });
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

  // v1.147 发送消息队列（§9.1）：移除 / 点击气泡回填编辑 / 冻结期手动续发。
  // 乐观更新本地队列 + 轮询兜底（500ms 内以 daemon 快照为权威覆盖）。
  async function removeQueued(m: QueuedMessage) {
    if (!sessionId) return;
    setQueue((prev) => prev.filter((x) => x.id !== m.id));
    try {
      await api.deleteQueuedMessage(sessionId, m.id);
    } catch {
      // 失败静默：下一轮轮询恢复权威态
    }
  }

  async function editQueued(m: QueuedMessage) {
    if (!sessionId || busy) return;
    setInputValue(m.text);
    inputRef.current?.focus();
    await removeQueued(m);
  }

  async function sendQueued(m: QueuedMessage) {
    if (!sessionId || busy || running) return;
    setBusy(true);
    try {
      // 先出队再发送：否则回合完成后 drain 会重复投递同一条
      await api.deleteQueuedMessage(sessionId, m.id).catch(() => {});
      setQueue((prev) => prev.filter((x) => x.id !== m.id));
      const r = await api.sendMessage(sessionId, m.text);
      if (!r.queued) setInput("");
    } catch {
      // 发送失败：文本回填输入框可重试（条目已出队，不重复投递）
      setInputValue(m.text);
    } finally {
      setBusy(false);
    }
  }

  // v1.127 消息级撤销：每个含改动的回合都可撤销——恢复到发送该消息前的工作区状态
  //（checkpoint 树级快照；该消息之后其他回合的改动随树一并回退）。unrevert 可撤销本次回滚。
  async function undoTurn(turn: Turn) {
    if (!sessionId || turn.firstPatchSeq === null || running) return;
    if (!window.confirm(t("thread.undo_confirm"))) return;
    try {
      const { checkpoints } = await api.checkpoints(sessionId);
      const cp = checkpoints.find((c) => c.event_seq === turn.firstPatchSeq);
      if (!cp) {
        window.alert(t("thread.undo_failed"));
        return;
      }
      // v1.135 撤销三合一：①任务文本回填输入框；②文件修改随树回滚；③线程截断
      //（服务端水位过滤，气泡及其后信息消失），整流重载。v1.136：撤销即终态，
      // 重做功能移除（检查点时间轴与 API 的 unrollback 能力保留）。
      await api.rollbackCheckpoint(cp.id, true);
      setInputValue(turn.task ?? "");
      setEvents([]);
      setStreamText("");
      setTraceEpoch((e) => e + 1);
    } catch {
      window.alert(t("thread.undo_failed"));
    }
  }

  const [copiedTurnId, setCopiedTurnId] = useState<number | null>(null);
  const copyTimer = useRef<number | null>(null);

  function copyTurn(turn: Turn) {
    if (copiedTurnId === turn.id) return;
    const text = turn.task ?? "";
    const markCopied = () => {
      setCopiedTurnId(turn.id);
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopiedTurnId(null), 1500);
    };
    // 剪贴板 API 不可用（局域网 http 非安全上下文）或被拒（WebView 未授权）时降级
    // execCommand——同步路径，点击手势内基本总能成功。
    const fallbackCopy = () => {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        if (document.execCommand("copy")) markCopied();
      } catch {
        // 两种通道都不可用：静默放弃（不留误导性「已复制」）
      } finally {
        ta.remove();
      }
    };
    if (navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(text).then(markCopied, fallbackCopy);
    } else {
      fallbackCopy();
    }
  }

  const running = RUNNING_STATES.has(status);
  const paused = status === "paused";

  // v1.109：回合分组——user_input 开新回合，其余事件进当前回合；model_delta 由 streamText 聚合不入列。
  const turns = useMemo(() => {
    const list: Turn[] = [];
    let cur: Turn | null = null;
    const blank = (): Turn => ({
      id: 0,
      task: null,
      items: [],
      firstPatchSeq: null,
      usage: { input: 0, output: 0, cached: 0, durationMs: 0 },
      model: null,
    });
    for (const ev of events) {
      if (ev.type === "user_input") {
        cur = {
          id: ev.id,
          task: String(ev.payload.text ?? ""),
          items: [],
          firstPatchSeq: null,
          usage: { input: 0, output: 0, cached: 0, durationMs: 0 },
          model: null,
        };
        list.push(cur);
        continue;
      }
      if (ev.type === "model_delta") continue;
      if (!cur) {
        cur = blank();
        list.push(cur);
      }
      // 消息级撤销锚点：回合内首个 patch_applied 的 checkpoint 记录该回合写入前状态。
      if (ev.type === "patch_applied" && cur.firstPatchSeq === null) {
        cur.firstPatchSeq = ev.seq;
      }
      // v1.129：decision 携带 usage——回合内多模型回合聚合（求和），页脚徽标消费。
      if (ev.type === "decision") {
        const u = ev.payload.usage as
          | { input_tokens?: number; output_tokens?: number; cached_input_tokens?: number; duration_ms?: number }
          | undefined;
        if (u) {
          cur.usage = {
            input: cur.usage.input + Number(u.input_tokens ?? 0),
            output: cur.usage.output + Number(u.output_tokens ?? 0),
            cached: cur.usage.cached + Number(u.cached_input_tokens ?? 0),
            durationMs: cur.usage.durationMs + Number(u.duration_ms ?? 0),
          };
        }
        // v1.131：回合模型标注——取回合内首个携带 model 的 decision（热切换 / fallback 后各回合如实）。
        if (cur.model === null) {
          const m = String(ev.payload.model ?? "");
          if (m) cur.model = m;
        }
      }
      cur.items.push(ev);
    }
    return list;
  }, [events]);

  // v1.127：每个含改动的回合（firstPatchSeq 非空）都提供撤销；重做钮挂在最近被撤销的回合上。

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
  // §7.5（v1.148）：常驻进度卡数据——全事件流最新一次 subtasks 快照（跨回合）。
  const liveSubtasks = useMemo(() => {
    for (let i = events.length - 1; i >= 0; i -= 1) {
      if (events[i].type !== "subtasks") continue;
      return Array.isArray(events[i].payload.items)
        ? (events[i].payload.items as Array<{ title?: unknown; status?: unknown }>)
        : [];
    }
    return [];
  }, [events]);


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
              {turn.task !== null && (
                <div className="turn-user" data-testid="turn-user">
                  <div className="turn-task">{turn.task}</div>
                  {/* v1.127 气泡下功能按钮组：复制（恒可用）/ 撤销（每个含改动回合）；v1.135 重做改挂「已撤销」提示条 */}
                  <div className="turn-actions">
                    <button
                      type="button"
                      className="turn-action"
                      data-testid={`turn-copy-${turn.id}`}
                      title={copiedTurnId === turn.id ? t("thread.copied") : t("thread.copy")}
                      aria-label={copiedTurnId === turn.id ? t("thread.copied") : t("thread.copy")}
                      onClick={() => copyTurn(turn)}
                    >
                      {copiedTurnId === turn.id ? t("thread.copied") : t("thread.copy")}
                    </button>
                    {turn.firstPatchSeq !== null && (
                      <button
                        type="button"
                        className="turn-action"
                        data-testid={`turn-undo-${turn.id}`}
                        disabled={running}
                        title={t("thread.undo_confirm")}
                        aria-label={t("thread.undo")}
                        onClick={() => void undoTurn(turn)}
                      >
                        ↩ {t("thread.undo")}
                      </button>
                    )}
                    {turn.model && (
                      <span className="turn-model" data-testid={`turn-model-${turn.id}`} title={turn.model}>
                        {turn.model}
                      </span>
                    )}
                  </div>
                </div>
              )}
              <div className="turn-body">
                <ReadOnlySummary items={turn.items} t={t} />
                {(() => {
                  // §9.2（v1.146）：回合内最新一次 subtasks 事件才渲染清单卡
                  const latestSubtasksId = [...turn.items]
                    .reverse()
                    .find((e) => e.type === "subtasks")?.id;
                  return turn.items.map((ev) => (
                    <EventNode
                      key={ev.id}
                      ev={ev}
                      t={t}
                      subtasksLatest={ev.id === latestSubtasksId}
                    />
                  ));
                })()}
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
                    {queue.length > 0 && (
                      <span className="queue-count" data-testid="queue-count">
                        {t("queue.count", { n: queue.length })}
                      </span>
                    )}
                  </div>
                )}
                <TurnUsageBadge turn={turn} t={t} />
              </div>
            </section>
          );
        })}
        {/* v1.147 发送消息队列（§9.1）：运行态入队的待发消息渲染为排队气泡——
            用户气泡同款居右样式 + 「已排队」徽标 + 移除钮；点击气泡文本回填输入框
           （即编辑重发）；冻结期（非运行态）条目显示发送钮可手动续发。 */}
        {queue.length > 0 && (
          <div className="queue-block" data-testid="queue-list">
            {queue.map((m) => (
              <div className="turn-user queue-item" key={m.id} data-testid={`queue-item-${m.id}`}>
                <button
                  type="button"
                  className="queue-text"
                  title={t("queue.edit_hint")}
                  onClick={() => void editQueued(m)}
                >
                  {m.text}
                </button>
                <div className="turn-actions">
                  <span className="queue-badge" data-testid="queue-badge">
                    {t("message.queued")}
                  </span>
                  {!running && (
                    <button
                      type="button"
                      className="turn-action"
                      data-testid={`queue-send-${m.id}`}
                      onClick={() => void sendQueued(m)}
                    >
                      {t("message.send")}
                    </button>
                  )}
                  <button
                    type="button"
                    className="turn-action"
                    data-testid={`queue-remove-${m.id}`}
                    title={t("queue.remove")}
                    aria-label={t("queue.remove")}
                    onClick={() => void removeQueued(m)}
                  >
                    ✕
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* §7.5（v1.148）：常驻进度卡——未完成清单钉在输入区上方，完成自动收起 */}
      <SubtasksLive items={liveSubtasks} t={t} />

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
          setInputValue(inputValue.trimEnd() ? `${inputValue.trimEnd()} @${path} ` : `@${path} `);
          inputRef.current?.focus();
        }}
      >
        {/* 任务上下文条（v1.126，参照 Codex / ZCode 输入上方选择器）：草稿态 = 项目 +
            工作区（主根 / 受管 worktree，即「分支选择」的 Tenon 对应物）两个选择器；
            会话态 = 只读标识（会话强绑定 project_id 不可切换）。 */}
        {showContext && (
          <div className="agent-context" data-testid="agent-context">
            {isDraft ? (
              <>
                <select
                  className="agent-context-select"
                  data-testid="draft-project-select"
                  aria-label={t("context.project")}
                  title={activeProject?.path}
                  value={projectId ?? ""}
                  disabled={!onSwitchDraftProject}
                  onChange={(e) => {
                    const target = projects?.find((p) => p.id === e.target.value);
                    if (target && target.id !== projectId) onSwitchDraftProject?.(target);
                  }}
                >
                  {projects!.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.display_name}
                    </option>
                  ))}
                </select>
                <select
                  className="agent-context-select"
                  data-testid="draft-workspace-select"
                  aria-label={t("context.workspace")}
                  value={draftWorktree ? "managed" : "main"}
                  disabled={!onChangeDraftWorktree}
                  onChange={(e) => onChangeDraftWorktree?.(e.target.value === "managed")}
                >
                  <option value="main">{t("workspace.main")}</option>
                  <option value="managed">{t("workspace.managed")}</option>
                </select>
              </>
            ) : (
              <>
                <span
                  className="agent-context-project"
                  data-testid="session-project-label"
                  title={activeProject?.path}
                >
                  {activeProject?.display_name ?? projectId}
                </span>
                {sessionWorktree && (
                  <span className="agent-context-wt" data-testid="session-worktree-badge">
                    ⎇ {t("workspace.managed")}
                  </span>
                )}
              </>
            )}
          </div>
        )}
        <textarea
          ref={inputRef}
          value={inputValue}
          placeholder={t("message.placeholder")}
          onChange={(e) => setInputValue(e.target.value)}
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
          {/* v1.147（§9.1）：运行态「发送」保留入队语义、与「停止」双钮并列——
              此前运行态唯一按钮变停止，消息无法发送（v1.59 形态）；暂停仍单钮恢复。 */}
          {paused ? (
            <button
              className="agent-send"
              onClick={resume}
              disabled={stopRequested || busy}
              data-testid="resume"
            >
              {t("message.resume")}
            </button>
          ) : (
            <>
              <button
                className="agent-send"
                onClick={send}
                disabled={(!sessionId && !(draft && onDraftSend)) || busy}
                data-testid="send"
              >
                {t("message.send")}
              </button>
              {running && (
                <button
                  className="agent-send agent-send-stop"
                  onClick={stop}
                  disabled={stopRequested}
                  data-testid="stop"
                >
                  {t("message.stop_short")}
                </button>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/** §9.2 A 级只读工具：不落步骤卡，按回合聚合为单行摘要（v1.112 降噪；明细见「轨迹」tab）。
 * laya_decide（v1.124）仅折叠步骤卡，计数与判定详情由 turn-laya 徽标承载。 */
const READ_ONLY_TOOLS = new Set(["read_file", "list_dir", "grep", "git_read", "lsp_query", "laya_decide", "subtasks"]);

function isReadOnlyStep(ev: EventItem): boolean {
  if (ev.type !== "patch_applied" && ev.type !== "command_run") return false;
  return READ_ONLY_TOOLS.has(String(ev.payload.tool ?? ""));
}

/** v1.112 降噪：回合内成功只读步骤聚合为单行动词计数（读取 2 · 搜索 1），详情见「轨迹」面板。 */
function ReadOnlySummary({ items, t }: { items: EventItem[]; t: Translate }) {
  const counts = new Map<string, number>();
  for (const ev of items) {
    if (!isReadOnlyStep(ev)) continue;
    if ((ev.payload.output as { ok?: boolean } | undefined)?.ok === false) continue;
    const tool = String(ev.payload.tool ?? "");
    // laya_decide 不进计数：判定结果由 EventNode 的 turn-laya 徽标展示
    if (tool === "laya_decide") continue;
    // subtasks 不进计数：清单状态由 EventNode 的 turn-subtasks 卡片展示（v1.146）
    if (tool === "subtasks") continue;
    counts.set(tool, (counts.get(tool) ?? 0) + 1);
  }
  if (counts.size === 0) return null;
  const label = [...counts.entries()].map(([tool, n]) => `${toolLabel(tool, t)} ${n}`).join(" · ");
  return (
    <div className="turn-readonly" data-testid="turn-readonly" title={t("thread.readonly_more")}>
      ⌕ {label}
    </div>
  );
}

/** v1.129：回合页脚模型用量徽标——↑input tok · 缓存命中 N% · N tok/s。
 * 回合内多模型回合聚合（Turn.usage 求和）；无 usage 数据 / 全零不渲染；
 * Anthropic 系不打 cache_control 时缓存未启用，cached=0 自然只显示速度段。 */
function TurnUsageBadge({ turn, t }: { turn: Turn; t: Translate }) {
  const u = turn.usage;
  if (!u || (u.input === 0 && u.output === 0)) return null;
  const parts: string[] = [`↑ ${u.input} tok`];
  if (u.cached > 0 && u.input > 0) {
    parts.push(`${t("thread.cached")} ${Math.round((u.cached / u.input) * 100)}%`);
  }
  if (u.durationMs > 0 && u.output > 0) {
    parts.push(`${(u.output / (u.durationMs / 1000)).toFixed(1)} tok/s`);
  }
  return (
    <div className="turn-usage" data-testid="turn-usage">
      {parts.join(" · ")}
    </div>
  );
}

/** §9.8 #4（v1.124）：agent 主动调用的 Laya 判定 → 单行轻量徽标（类型 → 结果 · 耗时）；
 * daemon 自动集成点（无 origin）维持不渲染，仅入「轨迹」。 */
function LayaBadge({ ev, t }: { ev: EventItem; t: Translate }) {
  if (ev.payload.origin !== "agent_tool") return null;
  const kind = String(ev.payload.kind ?? "");
  const duration = ev.payload.duration_ms;
  let result: string;
  if (ev.payload.fallback === true) {
    result = `${t("thread.model_fallback")} · ${String(ev.payload.reason ?? "")}`;
  } else {
    const r = ev.payload.result;
    result =
      kind === "choice"
        ? `${String((r as { label?: string })?.label ?? "")} (${Math.round(
            Number((r as { confidence?: number })?.confidence ?? 0) * 100,
          )}%)`
        : String(r);
  }
  const detail = [`${kind} → ${result}`, typeof duration === "number" ? `${duration}ms` : ""]
    .filter(Boolean)
    .join(" · ");
  return (
    <div className="turn-laya" data-testid="turn-laya">
      ◈ {t("thread.laya_badge")} · {detail}
    </div>
  );
}

/** §9.2（v1.146）：子任务清单卡——回合内最新一次 subtasks 事件的全量快照；
 * 标题行「子任务 · n/m」+ 状态行（SubtaskItems 共用）；全部完成灰显收敛。 */
function SubtasksCard({ ev, t }: { ev: EventItem; t: Translate }) {
  const items = Array.isArray(ev.payload.items)
    ? (ev.payload.items as Array<{ title?: unknown; status?: unknown }>)
    : [];
  if (items.length === 0) return null;
  const done = items.filter((i) => i.status === "done").length;
  const allDone = done === items.length;
  return (
    <div
      className={`turn-subtasks${allDone ? " turn-subtasks-done" : ""}`}
      data-testid="turn-subtasks"
      data-done={String(allDone)}
    >
      <div className="turn-subtasks-head">
        {t("thread.subtasks")} · {done}/{items.length}
      </div>
      <SubtaskItems items={items} />
    </div>
  );
}

/** 子任务状态行（v1.146 清单卡 / v1.148 常驻卡共用）：○ 待执行 / ▶ 进行中 / ✓ 完成。 */
function SubtaskItems({ items }: { items: Array<{ title?: unknown; status?: unknown }> }) {
  return (
    <ul className="turn-subtasks-list">
      {items.map((item, i) => {
        const status = String(item.status ?? "pending");
        const icon = status === "done" ? "✓" : status === "in_progress" ? "▶" : "○";
        return (
          <li key={i} className="turn-subtasks-item" data-status={status}>
            <span className="turn-subtasks-icon" aria-hidden="true">
              {icon}
            </span>
            <span className="turn-subtasks-title">{String(item.title ?? "")}</span>
          </li>
        );
      })}
    </ul>
  );
}

/** §7.5（v1.148）：常驻进度卡——会话最新一次 subtasks 快照钉在输入区上方，
 * 不随线程滚动，数据同源 500ms 事件轮询。未完成时常驻可见（头部可折叠，
 * 细进度条 + n/m）；全部完成自动收起（线程内 v1.146 历史卡保留）。 */
function SubtasksLive({
  items,
  t,
}: {
  items: Array<{ title?: unknown; status?: unknown }>;
  t: Translate;
}) {
  const [open, setOpen] = useState(true);
  const done = items.filter((i) => i.status === "done").length;
  if (items.length === 0 || done === items.length) return null;
  const pct = Math.round((done / items.length) * 100);
  return (
    <div className="subtasks-live" data-testid="subtasks-live" data-open={String(open)}>
      <button
        type="button"
        className="subtasks-live-head"
        data-testid="subtasks-live-head"
        aria-expanded={open}
        title={t("thread.subtasks")}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="subtasks-live-bar" aria-hidden="true">
          <span style={{ width: `${pct}%` }} />
        </span>
        <span className="subtasks-live-label">
          {t("thread.subtasks")} · {done}/{items.length}
        </span>
        <span className="subtasks-live-caret" aria-hidden="true">
          {open ? "▾" : "▸"}
        </span>
      </button>
      {open && <SubtaskItems items={items} />}
    </div>
  );
}

/** 事件 → 会话流节点；只渲染面向用户的子集，完整事件表由底部「轨迹」tab 承载。 */
function EventNode({
  ev,
  t,
  subtasksLatest,
}: {
  ev: EventItem;
  t: Translate;
  subtasksLatest: boolean;
}) {
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
      // v1.112：成功的只读步骤聚合为单行（回合渲染层），失败的仍单独红显。
      if (isReadOnlyStep(ev) && (ev.payload.output as { ok?: boolean } | undefined)?.ok !== false) {
        return null;
      }
      return <StepCard ev={ev} t={t} risk={false} />;
    case "direct_action":
      return <StepCard ev={ev} t={t} risk={true} />;
    case "diagnostics": {
      if ((ev.payload as { dirty_conflict?: boolean }).dirty_conflict) return null;
      const verification = String(
        (ev.payload as { verification?: string }).verification ?? "",
      ).trim();
      if (!verification) return null;
      return (
        <div className="turn-verify">
          <strong>{t("evidence.title")}</strong>
          <pre>{verification}</pre>
        </div>
      );
    }
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
    case "decider_call":
      // §9.8 #4（v1.124）：agent 经 laya_decide 主动调用 → 轻量徽标；
      // daemon 自动集成点与未知来源不渲染（仅「轨迹」可见）
      return <LayaBadge ev={ev} t={t} />;
    case "subtasks":
      // §9.2（v1.146）：子任务清单卡——每回合仅最新一次快照渲染，
      // 同回合更早的状态演进入「轨迹」面板
      if (!subtasksLatest) return null;
      return <SubtasksCard ev={ev} t={t} />;
    default:
      // sensing / checkpoint / session_title / 降级 / 压缩 / 记忆 / 未知：不渲染
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
