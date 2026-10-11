// v2.8 审查摘要卡（design.md §7.2 / design-v2.md §6）纯函数契约：
// 事件流 → digest——计划取最新 plan_submitted、子任务取最新 subtasks 快照
// （跨回合覆盖）、产物 changed_files 去重序时（上限 8、旧者挤出）、证据取最新
// 非空 verification；空事件流 = 空 digest（会话切换自然复位）。
import { describe, expect, it } from "vitest";
import {
  ARTIFACTS_LIMIT,
  digestFromEvents,
  digestHasContent,
  EMPTY_REVIEW_DIGEST,
} from "../lib/reviewDigest";

describe("digestFromEvents（v2.8）", () => {
  it("空事件流得空 digest（无任何区块）", () => {
    expect(digestFromEvents([])).toEqual(EMPTY_REVIEW_DIGEST);
    expect(digestHasContent(EMPTY_REVIEW_DIGEST)).toBe(false);
  });

  it("计划取最新 plan_submitted（跨回合覆盖）", () => {
    const digest = digestFromEvents([
      { type: "plan_submitted", payload: { items: ["第一步"] } },
      { type: "plan_submitted", payload: { items: ["重拟的步骤一", "步骤二"] } },
    ]);
    expect(digest.plan).toEqual(["重拟的步骤一", "步骤二"]);
    expect(digestHasContent(digest)).toBe(true);
  });

  it("子任务取最新 subtasks 全量快照并归一 title/status", () => {
    const digest = digestFromEvents([
      { type: "subtasks", payload: { items: [{ title: "a", status: "done" }] } },
      {
        type: "subtasks",
        payload: {
          items: [
            { title: "扫描", status: "done" },
            { title: "修复", status: "in_progress" },
            { status: "pending" },
          ],
        },
      },
    ]);
    expect(digest.subtasks).toEqual([
      { title: "扫描", status: "done" },
      { title: "修复", status: "in_progress" },
      { title: "", status: "pending" },
    ]);
  });

  it("产物 changed_files 去重 + 上限 8（旧者挤出、挤出后再改重新入列）", () => {
    // 在列重复：不追加（独立小场景，避免与上限挤出相互掩盖）
    const dedup = digestFromEvents([
      { type: "patch_applied", payload: { output: { changed_files: ["dup.ts"] } } },
      { type: "patch_applied", payload: { output: { changed_files: ["dup.ts"] } } },
    ]);
    expect(dedup.artifacts).toEqual(["dup.ts"]);
    // 上限 8：旧者挤出；挤出后再改的文件重新入列（「最近改动」语义）
    const events = Array.from({ length: 12 }, (_, i) => ({
      type: "patch_applied",
      payload: { output: { changed_files: [`f${i}.ts`] } },
    }));
    const digest = digestFromEvents([
      ...events,
      { type: "patch_applied", payload: { output: { changed_files: ["f0.ts"] } } },
    ]);
    expect(digest.artifacts).toHaveLength(ARTIFACTS_LIMIT);
    expect(digest.artifacts[digest.artifacts.length - 1]).toBe("f0.ts");
    expect(digest.artifacts).not.toContain("f4.ts"); // 最旧者恰被挤出
  });

  it("证据取最新非空 verification；空串与缺失不覆盖", () => {
    const digest = digestFromEvents([
      { type: "diagnostics", payload: { verification: "  " } },
      { type: "diagnostics", payload: { verification: "首轮验证：通过（无诊断）" } },
      { type: "diagnostics", payload: {} },
    ]);
    expect(digest.evidence).toBe("首轮验证：通过（无诊断）");
  });

  it("dirty_conflict 形态的 diagnostics（无 verification）不产生证据", () => {
    const digest = digestFromEvents([
      { type: "diagnostics", payload: { dirty_conflict: true } },
    ]);
    expect(digest.evidence).toBeNull();
    expect(digestHasContent(digest)).toBe(false);
  });

  it("v2.9 context_usage：最新快照覆盖、字段归一、缺 breakdown/totals 容错跳过", () => {
    const gaugeEvent = (over: Record<string, unknown>) => ({
      type: "context_usage",
      payload: {
        breakdown: { system: 900, l1: 100, history: 250, l5: 50, tools: 0 },
        est_total: 1300,
        totals: { input_tokens: 1400, output_tokens: 60, cost_usd: 0.0123 },
        ...over,
      },
    });
    let digest = digestFromEvents([gaugeEvent({})]);
    expect(digest.context).toEqual({
      breakdown: { system: 900, l1: 100, history: 250, l5: 50, tools: 0 },
      estTotal: 1300,
      totals: { inputTokens: 1400, outputTokens: 60, costUsd: 0.0123 },
    });
    expect(digestHasContent(digest)).toBe(true);
    // 第二轮覆盖第一轮（仪表显示最新构成）
    digest = digestFromEvents([
      gaugeEvent({}),
      gaugeEvent({ breakdown: { system: 800, l1: 0, history: 400, l5: 0, tools: 200 }, est_total: 1400 }),
    ]);
    expect(digest.context!.breakdown.tools).toBe(200);
    // 缺段容错：不产生仪表（宁缺勿错）
    digest = digestFromEvents([{ type: "context_usage", payload: { est_total: 100 } }]);
    expect(digest.context).toBeNull();
    expect(digestHasContent(digest)).toBe(false);
  });
});
