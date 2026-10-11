// v2.8 审查摘要卡（design.md §7.2 / design-v2.md §6「计划/子任务与证据/产物卡」）：
// 会话事件流 → 审查面 digest 纯汇总——计划（最新 plan_submitted 清单）、子任务
// （最新 subtasks 全量快照）、产物（patch_applied.changed_files 去重序时）、证据
// （最新 diagnostics.verification）。事件流为空（会话切换 / 草稿态）自然得空 digest。
// 「最新」取尾扫描（与 v1.148 liveSubtasks 同语义：全事件流跨回合最新一次快照）。

export interface ReviewSubtaskItem {
  title: string;
  status: string;
}

/** v2.9 运行仪表（design-v2.md §5③）：最新一次 context_usage 事件的快照。 */
export interface ReviewContextGauge {
  breakdown: {
    system: number;
    l1: number;
    history: number;
    l5: number;
    tools: number;
  };
  estTotal: number;
  totals: { inputTokens: number; outputTokens: number; costUsd: number };
}

export interface ReviewDigest {
  /** 最新 plan_submitted 的计划清单；null = 无计划。 */
  plan: string[] | null;
  /** 最新 subtasks 全量快照；空数组 = 无清单。 */
  subtasks: ReviewSubtaskItem[];
  /** 改动产物清单（去重、最新在后，上限 8 条）。 */
  artifacts: string[];
  /** 最新验证结论（diagnostics.verification 非空）；null = 尚无。 */
  evidence: string | null;
  /** v2.9 运行仪表（上下文构成 + 会话累计成本）；null = 尚无模型调用。 */
  context: ReviewContextGauge | null;
}

export const EMPTY_REVIEW_DIGEST: ReviewDigest = {
  plan: null,
  subtasks: [],
  artifacts: [],
  evidence: null,
  context: null,
};

export const ARTIFACTS_LIMIT = 8;
/** 证据文本截断上限（验证结论通常一两行，防御超长输出挤占审查面）。 */
export const EVIDENCE_LIMIT = 400;

interface DigestEvent {
  type: string;
  payload: Record<string, unknown>;
}

/** 从会话事件流（trace 顺序，旧→新）汇总审查 digest。 */
export function digestFromEvents(events: DigestEvent[]): ReviewDigest {
  let plan: string[] | null = null;
  let subtasks: ReviewSubtaskItem[] = [];
  let evidence: string | null = null;
  let context: ReviewContextGauge | null = null;
  const artifacts: string[] = [];
  for (const ev of events) {
    if (ev.type === "plan_submitted") {
      plan = Array.isArray(ev.payload.items)
        ? (ev.payload.items as unknown[]).map((item) => String(item))
        : [];
    } else if (ev.type === "subtasks") {
      subtasks = Array.isArray(ev.payload.items)
        ? (ev.payload.items as Array<{ title?: unknown; status?: unknown }>).map((item) => ({
            title: String(item.title ?? ""),
            status: String(item.status ?? "pending"),
          }))
        : [];
    } else if (ev.type === "diagnostics") {
      const text = String(ev.payload.verification ?? "").trim();
      if (text) evidence = text;
    } else if (ev.type === "context_usage") {
      const b = ev.payload.breakdown as Record<string, unknown> | undefined;
      const totals = ev.payload.totals as Record<string, unknown> | undefined;
      if (b && totals) {
        const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
        context = {
          breakdown: {
            system: num(b.system),
            l1: num(b.l1),
            history: num(b.history),
            l5: num(b.l5),
            tools: num(b.tools),
          },
          estTotal: num(ev.payload.est_total),
          totals: {
            inputTokens: num(totals.input_tokens),
            outputTokens: num(totals.output_tokens),
            costUsd: num(totals.cost_usd),
          },
        };
      }
    } else if (ev.type === "patch_applied") {
      const changed = (ev.payload.output as { changed_files?: unknown } | undefined)?.changed_files;
      if (!Array.isArray(changed)) continue;
      for (const raw of changed) {
        const file = String(raw);
        if (!file || artifacts.includes(file)) continue;
        if (artifacts.length >= ARTIFACTS_LIMIT) artifacts.shift();
        artifacts.push(file);
      }
    }
  }
  return { plan, subtasks, artifacts, evidence, context };
}

/** digest 是否含任何可渲染区块（摘要区整体是否渲染的判据）。 */
export function digestHasContent(digest: ReviewDigest): boolean {
  return (
    (digest.plan !== null && digest.plan.length > 0) ||
    digest.subtasks.length > 0 ||
    digest.artifacts.length > 0 ||
    digest.evidence !== null ||
    digest.context !== null
  );
}
