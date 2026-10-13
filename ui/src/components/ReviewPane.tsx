// v2.4 常驻审查面（design-v2.md §6「审查面是第二主区」）：会话运行中自动停靠
// 右栏，实时呈现代理改动 diff 流——「人改与 AI 改实时互见」从口号变默认形态。
// 空闲自动收起；用户手动关闭后不再自动弹出（会话切换重置）。
// v2.7 diff 级反馈回路：diff 行点选（DiffPanel selectable）→ 反馈卡
// （文件:行上下文 + 反馈输入）→ 任务文本经行内指令同款链路注入当前会话；
// 运行态发送自动入队（v1.147），回合自然完成后续跑消费——「边跑边审、审完即改」。
// v2.8 审查摘要卡（design-v2.md §6「计划/子任务与证据/产物卡」）：diff 流上方
// 常驻摘要区——计划 / 子任务进度 / 改动产物 / 验证证据（对齐 Codex summary pane）。
// v2.9 运行仪表（design-v2.md §5③「上下文观测」+ §6）：摘要区顶部——输入构成
// 五桶（系统提示/L1 工作集/历史对话/L5 跨会话记忆/工具输出）堆叠占比条 +
// 图例 tok 数 + 会话累计成本；数据源 context_usage 事件（每轮模型调用后发）。
// v2.38 工具面板（ZCode 式对话工作台）：flag toolPanelTabs 开启时审查面升级
// tab 化——diff（默认页：diff 流 + 反馈回路原样）/ 文件（FileTree 树复用侧栏
// 源码区数据链 + Monaco 只读预览 +「编辑器打开」跳浮层）/ 概览（摘要五卡 +
// 运行仪表整体迁入）；页记忆 tenon:toolTab。不传 toolTabs 即 v2.5 单一 diff
// 审查面回退形态（无 tab 栏、摘要卡常驻顶部），渲染路径与 v2.36 完全一致。
import { useEffect, useMemo, useState } from "react";
import Editor from "@monaco-editor/react";
import { DiffPanel, parseDiffLines } from "./DiffPanel";
import { FileTree } from "./FileTree";
import { useResolvedTheme } from "./EditorPane";
import { mapDiffLines, selectionTargetFromInfos, type DiffSelectionTarget } from "../lib/diffSelect";
import {
  digestHasContent,
  EVIDENCE_LIMIT,
  type ReviewContextGauge,
  type ReviewDigest,
  type ReviewSubtaskItem,
} from "../lib/reviewDigest";
import type { Translate } from "../lib/i18n";
import type { TenonApi } from "../lib/api";

/** v2.38 工具面板三页。 */
export type ToolTab = "diff" | "files" | "overview";

/** v2.38 files 页数据链（App 仅在 flag toolPanelTabs on 时传入）。 */
export interface ToolTabsProps {
  api: TenonApi;
  projectId: string | null;
  /** ProjectRuntime 文件事件版本；变化即刷新树（§6.4 / §8.1）。 */
  refreshToken?: number;
  /** 「编辑器打开」——跳既有全屏编辑器浮层（多标签编辑能力不丢）。 */
  onOpenEditor: (path: string) => void;
}

const TOOL_TAB_KEY = "tenon:toolTab";

function readToolTab(): ToolTab {
  try {
    const raw = localStorage.getItem(TOOL_TAB_KEY);
    if (raw === "files" || raw === "overview") return raw;
  } catch {
    // 存储不可用：回默认页
  }
  return "diff";
}

/** 只读预览语言：扩展名 → monaco 语言 id（缺省 plaintext）。 */
const EXT_LANG: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  css: "css",
  html: "html",
  md: "markdown",
  py: "python",
  rs: "rust",
  go: "go",
  java: "java",
  c: "c",
  h: "c",
  cpp: "cpp",
  hpp: "cpp",
  rb: "ruby",
  php: "php",
  sh: "shell",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  sql: "sql",
};

function langOf(path: string): string {
  return EXT_LANG[path.split(".").pop() ?? ""] ?? "plaintext";
}

/** v2.38 files 页只读预览：api.readFile 载入 + Monaco readOnly（tenon 双主题）。 */
function FilePreview({
  api,
  projectId,
  path,
  onOpenEditor,
  t,
}: {
  api: TenonApi;
  projectId: string | null;
  path: string;
  onOpenEditor: (path: string) => void;
  t: Translate;
}) {
  const theme = useResolvedTheme();
  const [content, setContent] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    setContent(null);
    setFailed(false);
    if (!projectId) return;
    api
      .readFile(projectId, path)
      .then((r) => {
        if (alive) setContent(r.content);
      })
      .catch(() => {
        if (alive) setFailed(true);
      });
    return () => {
      alive = false;
    };
  }, [api, projectId, path]);

  return (
    <div className="review-preview" data-testid="review-preview">
      <div className="review-preview-head">
        <span className="review-preview-path" title={path}>
          {path}
        </span>
        <button
          type="button"
          className="review-preview-open"
          data-testid="review-preview-open"
          disabled={!projectId || failed}
          onClick={() => onOpenEditor(path)}
        >
          {t("review.open_editor")}
        </button>
      </div>
      <div className="review-preview-body">
        {failed ? (
          <div className="review-files-empty" data-testid="review-preview-error">
            {t("review.files_error")}
          </div>
        ) : (
          <Editor
            value={content ?? ""}
            language={langOf(path)}
            theme={theme === "light" ? "tenon-light" : "tenon-dark"}
            options={{
              readOnly: true,
              minimap: { enabled: false },
              scrollBeyondLastLine: false,
              fontSize: 12,
              automaticLayout: true,
            }}
          />
        )}
      </div>
    </div>
  );
}

/** v2.9 运行仪表：五桶水平堆叠占比条（宽 0 时仍渲染 1px 保序）。 */
function ReviewContextBar({
  breakdown,
  total,
}: {
  breakdown: ReviewContextGauge["breakdown"];
  total: number;
}) {
  const segments = (
    [
      ["system", breakdown.system],
      ["l1", breakdown.l1],
      ["history", breakdown.history],
      ["l5", breakdown.l5],
      ["tools", breakdown.tools],
    ] as const
  ).map(([key, value]) => ({
    key,
    // 占比分母为 est_total（缺省回退五桶之和），容错 0 除
    pct: total > 0 ? (value / total) * 100 : 0,
  }));
  return (
    <div className="review-ctx-bar" data-testid="review-ctx-bar" role="img" aria-label="context composition">
      {segments.map((seg) => (
        <span
          key={seg.key}
          className={`review-ctx-seg seg-${seg.key}`}
          style={{ width: seg.pct > 0 ? `${seg.pct}%` : "1px" }}
        />
      ))}
    </div>
  );
}

/** 子任务状态行（v2.8）：复用 v1.146 状态语义 ○ 待执行 / ▶ 进行中 / ✓ 完成。 */
function ReviewSubtaskRow({ item }: { item: ReviewSubtaskItem }) {
  const icon = item.status === "done" ? "✓" : item.status === "in_progress" ? "▶" : "○";
  return (
    <li data-status={item.status}>
      <span className="review-subtask-icon" aria-hidden="true">
        {icon}
      </span>
      <span className="review-subtask-title">{item.title}</span>
    </li>
  );
}

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

/** v2.8 摘要五卡 + v2.9 运行仪表：回退形态常驻 diff 流上方，工具面板迁「概览」页。 */
function DigestSection({ digest, t }: { digest: ReviewDigest; t: Translate }) {
  if (!digest || !digestHasContent(digest)) return null;
  return (
    <div className="review-digest" data-testid="review-digest">
      {digest.context && (
        <section className="review-digest-block" data-testid="review-gauges">
          <h4>{t("review.ctx_title")}</h4>
          <ReviewContextBar breakdown={digest.context.breakdown} total={digest.context.estTotal} />
          <ul className="review-ctx-legend" data-testid="review-ctx-legend">
            {(
              [
                ["system", "review.ctx_system"],
                ["l1", "review.ctx_l1"],
                ["history", "review.ctx_history"],
                ["l5", "review.ctx_l5"],
                ["tools", "review.ctx_tools"],
              ] as const
            ).map(([key, label]) => (
              <li key={key}>
                <span className={`review-ctx-dot seg-${key}`} aria-hidden />
                {t(label)} <span className="review-ctx-tok">{digest.context!.breakdown[key]}</span>
              </li>
            ))}
          </ul>
          <div className="review-ctx-cost" data-testid="review-ctx-cost">
            <span>
              {t("review.cost_in")} {digest.context.totals.inputTokens} ·{" "}
              {t("review.cost_out")} {digest.context.totals.outputTokens}
            </span>
            <span>
              {t("review.cost_total")} ${digest.context.totals.costUsd.toFixed(4)}
            </span>
          </div>
        </section>
      )}
      {digest.plan && digest.plan.length > 0 && (
        <section className="review-digest-block" data-testid="review-plan">
          <h4>{t("review.plan_title")}</h4>
          <ol>
            {digest.plan.map((item, i) => (
              <li key={i}>{item}</li>
            ))}
          </ol>
        </section>
      )}
      {digest.subtasks.length > 0 && (
        <section className="review-digest-block" data-testid="review-subtasks">
          <h4>
            {t("review.subtasks_title")}
            <span className="review-digest-count">
              {" "}
              {digest.subtasks.filter((s) => s.status === "done").length}/{digest.subtasks.length}
            </span>
          </h4>
          <ul>
            {digest.subtasks.map((item, i) => (
              <ReviewSubtaskRow key={i} item={item} />
            ))}
          </ul>
        </section>
      )}
      {digest.artifacts.length > 0 && (
        <section className="review-digest-block" data-testid="review-artifacts">
          <h4>{t("review.artifacts_title")}</h4>
          <ul>
            {digest.artifacts.map((file) => (
              <li key={file} className="review-digest-file">
                {file}
              </li>
            ))}
          </ul>
        </section>
      )}
      {digest.evidence !== null && (
        <section className="review-digest-block" data-testid="review-evidence">
          <h4>{t("review.evidence_title")}</h4>
          <pre>{digest.evidence.slice(0, EVIDENCE_LIMIT)}</pre>
        </section>
      )}
    </div>
  );
}

export function ReviewPane({
  t,
  diff,
  running,
  statusLabel,
  onClose,
  onFeedback,
  digest,
  width,
  floating,
  toolTabs,
}: {
  t: Translate;
  diff: string | null;
  running: boolean;
  statusLabel: string;
  onClose: () => void;
  /** v2.7 反馈发送：入参为组装完成的任务文本（App 经行内指令同款链路注入会话）。 */
  onFeedback?: (task: string) => void;
  /** v2.8 审查摘要（AgentPanel 事件流汇总上提）：计划 / 子任务 / 产物 / 证据。 */
  digest?: ReviewDigest;
  /** v2.11 宽度拖拽记忆（design-v2.md §6）：App 持有并持久化的像素宽（280-520）。 */
  width?: number;
  /** v2.52 窄屏手动开启：渲染为右缘浮层（zone-float，同侧栏浮层语言）。 */
  floating?: boolean;
  /** v2.38 工具面板（flag toolPanelTabs）：缺席 = v2.5 单一 diff 审查面回退形态。 */
  toolTabs?: ToolTabsProps;
}) {
  const [range, setRange] = useState<{ start: number; end: number } | null>(null);
  const [instruction, setInstruction] = useState("");
  // v2.38 工具面板页状态：记忆落盘（白名单校验，非法回 diff）
  const [tab, setTab] = useState<ToolTab>(readToolTab);
  // files 页预览目标；会话 / 项目切换时清空（避免串读旧路径）
  const [previewPath, setPreviewPath] = useState<string | null>(null);

  useEffect(() => {
    setPreviewPath(null);
  }, [toolTabs?.projectId]);

  const switchTab = (next: ToolTab) => {
    setTab(next);
    try {
      localStorage.setItem(TOOL_TAB_KEY, next);
    } catch {
      // 存储不可用：仅当前会话生效
    }
  };

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

  const digestNode = digest ? <DigestSection digest={digest} t={t} /> : null;
  const feedbackNode = target && (
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
            return;
          }
          // v2.33：Enter 直发（Shift+Enter 换行；IME 组合态仅确认候选不发送）。
          if (
            e.key === "Enter" &&
            !e.shiftKey &&
            !e.altKey &&
            !e.nativeEvent.isComposing &&
            e.keyCode !== 229
          ) {
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
  );

  return (
    <aside
      className={`zone zone-review${floating ? " zone-float" : ""}`}
      data-testid="review-pane"
      data-running={running ? "1" : "0"}
      style={width !== undefined ? { width } : undefined}
    >
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
      {toolTabs && (
        <div className="review-tabs" role="tablist" data-testid="review-tabs">
          {(["diff", "files", "overview"] as const).map((key) => (
            <button
              key={key}
              type="button"
              role="tab"
              aria-selected={tab === key}
              className={tab === key ? "review-tab active" : "review-tab"}
              data-testid={`review-tab-${key}`}
              onClick={() => switchTab(key)}
            >
              {t(
                key === "diff"
                  ? "review.tab_diff"
                  : key === "files"
                    ? "review.tab_files"
                    : "review.tab_overview",
              )}
            </button>
          ))}
        </div>
      )}
      <div className="review-body">
        {!toolTabs && (
          <>
            {digestNode}
            {feedbackNode}
            <DiffPanel
              diff={diff}
              emptyText={running ? t("review.empty_running") : t("review.empty")}
              selectable={Boolean(onFeedback)}
              selection={range}
              onSelectionChange={setRange}
            />
          </>
        )}
        {toolTabs && tab === "diff" && (
          <>
            {feedbackNode}
            <DiffPanel
              diff={diff}
              emptyText={running ? t("review.empty_running") : t("review.empty")}
              selectable={Boolean(onFeedback)}
              selection={range}
              onSelectionChange={setRange}
            />
          </>
        )}
        {toolTabs && tab === "overview" && digestNode}
        {toolTabs && tab === "files" && (
          <div className="review-files" data-testid="review-files">
            <div className="review-files-tree">
              {toolTabs.projectId ? (
                <FileTree
                  api={toolTabs.api}
                  t={t}
                  projectId={toolTabs.projectId}
                  refreshToken={toolTabs.refreshToken}
                  onOpenFile={setPreviewPath}
                />
              ) : (
                <div className="review-files-empty" data-testid="review-files-empty">
                  {t("review.files_empty")}
                </div>
              )}
            </div>
            {previewPath && (
              <FilePreview
                api={toolTabs.api}
                projectId={toolTabs.projectId}
                path={previewPath}
                onOpenEditor={toolTabs.onOpenEditor}
                t={t}
              />
            )}
          </div>
        )}
      </div>
    </aside>
  );
}
