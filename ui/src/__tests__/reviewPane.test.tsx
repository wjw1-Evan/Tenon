// v2.4 常驻审查面（design-v2.md §6）UI 契约：
// ① ReviewPane 组件：运行中/空闲状态点与状态标签、diff 渲染（复用 DiffPanel
//    虚拟滚动）、空态文案、✕ 关闭回调；
// ② App 接线：运行态（RUNNING_STATES）自动停靠、空闲自动收起、手动关闭后
//    本次会话不再自动弹（rerender 保持），关闭回调触发。
// v2.7 diff 级反馈回路：行点选 → 反馈卡（上下文标签 + 反馈输入）→ 发送组装
// 任务（前缀 + 摘录 + 约束）→ 选区清空；纯元数据行选区不出卡。
// v2.8 审查摘要卡：digest（计划/子任务/产物/证据）注入渲染、空 digest 不渲染。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ReviewPane, buildReviewTask } from "../components/ReviewPane";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (k in Object.fromEntries(backing) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
});

const t = (k: string) => k;

describe("ReviewPane 组件（design-v2.md §6）", () => {
  it("renders running state with diff content", () => {
    render(
      <ReviewPane
        t={t}
        diff={"--- a/f.ts\n+++ b/f.ts\n@@ -1 +1 @@\n-old\n+new"}
        running
        statusLabel="review.running"
        onClose={() => {}}
      />
    );
    expect(screen.getByTestId("review-pane").getAttribute("data-running")).toBe("1");
    expect(screen.getByTestId("review-status").textContent).toBe("review.running");
    // diff 行渲染（DiffPanel 虚拟滚动物化可视行）
    expect(screen.getByText(/review\.title/)).toBeTruthy();
    expect(screen.getByText("+new")).toBeTruthy();
  });

  it("idle state shows idle status and empty text", () => {
    render(
      <ReviewPane t={t} diff={null} running={false} statusLabel="review.idle" onClose={() => {}} />
    );
    expect(screen.getByTestId("review-pane").getAttribute("data-running")).toBe("0");
    expect(screen.getByText("review.idle")).toBeTruthy();
    expect(screen.getByText("review.empty")).toBeTruthy();
  });

  it("close button fires onClose", () => {
    const onClose = vi.fn();
    render(
      <ReviewPane t={t} diff={null} running onClose={onClose} statusLabel="review.running" />
    );
    fireEvent.click(screen.getByTestId("review-close"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("ReviewPane diff 级反馈回路（v2.7，design-v2.md §6）", () => {
  // 行序：0 --- 1 +++ 2 @@ 3 ctx(新1) 4 -old 5 +new(新2) 6 ctx2(新3)
  const DIFF = "--- a/f.ts\n+++ b/f.ts\n@@ -1,3 +1,3 @@\n ctx\n-old\n+new\n ctx2";

  it("选中 diff 行浮出反馈卡：发送组装任务（键前缀 + 摘录 + 约束）并清空选区", () => {
    const onFeedback = vi.fn();
    render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        onFeedback={onFeedback}
      />
    );
    fireEvent.click(screen.getAllByTestId("diff-line")[5]);
    expect(screen.getByTestId("review-feedback")).toBeTruthy();
    expect(screen.getByTestId("review-fb-target").textContent).toBe("review.fb_target");
    fireEvent.change(screen.getByTestId("review-fb-input"), { target: { value: "改用更明确的命名" } });
    fireEvent.click(screen.getByTestId("review-fb-send"));
    expect(onFeedback).toHaveBeenCalledTimes(1);
    const task = onFeedback.mock.calls[0][0] as string;
    expect(task).toContain("review.fb_task_prefix");
    expect(task).toContain("+new");
    expect(task).toContain("review.fb_task_constraint");
    // 发送后选区与反馈卡一并清空
    expect(screen.queryByTestId("review-feedback")).toBeNull();
    expect(screen.getAllByTestId("diff-line")[5].getAttribute("data-selected")).toBeNull();
  });

  it("纯元数据 / hunk 头行选区不出反馈卡；✕ 取消选区不入队", () => {
    const onFeedback = vi.fn();
    render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        onFeedback={onFeedback}
      />
    );
    fireEvent.click(screen.getAllByTestId("diff-line")[2]);
    expect(screen.queryByTestId("review-feedback")).toBeNull();
    fireEvent.click(screen.getAllByTestId("diff-line")[4]);
    expect(screen.getByTestId("review-feedback")).toBeTruthy();
    fireEvent.click(screen.getByTestId("review-fb-clear"));
    expect(screen.queryByTestId("review-feedback")).toBeNull();
    expect(onFeedback).not.toHaveBeenCalled();
  });

  it("空输入禁发；运行态显示入队提示", () => {
    render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        onFeedback={() => {}}
      />
    );
    fireEvent.click(screen.getAllByTestId("diff-line")[3]);
    expect((screen.getByTestId("review-fb-send") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("review-feedback").textContent).toContain("review.fb_hint_queued");
  });
});

describe("buildReviewTask 任务组装（v2.7）", () => {
  const table: Record<string, string> = {
    "review.fb_task_prefix": "审查反馈：针对 {path} {range}的最新改动——{instruction}",
    "review.fb_task_range": "第 {start}-{end} 行",
    "review.fb_task_file": "（整文件改动）",
    "review.fb_task_excerpt": "改动摘录：",
    "review.fb_task_constraint": "约束：最小改动。",
  };
  const t = (k: string, vars?: Record<string, string | number>) => {
    let s = table[k] ?? k;
    if (vars) for (const [kk, vv] of Object.entries(vars)) s = s.replaceAll(`{${kk}}`, String(vv));
    return s;
  };

  it("行级目标：前缀含路径行区与指令，摘录围栏，约束收尾", () => {
    const task = buildReviewTask("命名更明确", { path: "src/a.ts", startLine: 2, endLine: 3, excerpt: "+x\n-y" }, t);
    expect(task).toContain("针对 src/a.ts 第 2-3 行的最新改动——命名更明确");
    expect(task).toContain("改动摘录：\n```diff\n+x\n-y\n```");
    expect(task).toContain("约束：最小改动。");
  });

  it("文件级目标（纯删除选区）：行区退化为整文件占位", () => {
    const task = buildReviewTask("这个文件不该删", { path: "gone.ts", excerpt: "-a" }, t);
    expect(task).toContain("针对 gone.ts （整文件改动）的最新改动——这个文件不该删");
  });
});

describe("ReviewPane 审查摘要卡（v2.8，design-v2.md §6）", () => {
  const DIFF = "--- a/f.ts\n+++ b/f.ts\n@@ -1 +1 @@\n-old\n+new";

  it("digest 注入渲染：计划清单 / 子任务状态与计数 / 产物 / 证据截断", () => {
    render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        digest={{
          plan: ["扫描调用点", "替换 API", "回归验证"],
          subtasks: [
            { title: "扫描调用点", status: "done" },
            { title: "替换 API", status: "in_progress" },
            { title: "回归验证", status: "pending" },
          ],
          artifacts: ["src/api.ts", "src/api.test.ts"],
          evidence: "验证通过（无新诊断）",
          context: null,
        }}
      />
    );
    const digest = screen.getByTestId("review-digest");
    expect(digest).toBeTruthy();
    const plan = screen.getByTestId("review-plan");
    expect(plan.querySelectorAll("li")).toHaveLength(3);
    expect(plan).toHaveTextContent("替换 API");
    const subtasks = screen.getByTestId("review-subtasks");
    expect(subtasks).toHaveTextContent("1/3");
    expect(subtasks.querySelector('li[data-status="in_progress"]')).toBeTruthy();
    expect(screen.getByTestId("review-artifacts")).toHaveTextContent("src/api.test.ts");
    expect(screen.getByTestId("review-evidence")).toHaveTextContent("验证通过（无新诊断）");
  });

  it("证据超长截断至 EVIDENCE_LIMIT", () => {
    render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        digest={{ plan: null, subtasks: [], artifacts: [], evidence: "x".repeat(1000), context: null }}
      />
    );
    expect(screen.getByTestId("review-evidence").querySelector("pre")?.textContent).toHaveLength(400);
  });

  it("空 digest（无任一区块数据）不渲染摘要区；计划空数组不渲染计划块", () => {
    const { rerender } = render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        digest={{ plan: null, subtasks: [], artifacts: [], evidence: null, context: null }}
      />
    );
    expect(screen.queryByTestId("review-digest")).toBeNull();
    rerender(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        digest={{ plan: [], subtasks: [], artifacts: ["only.ts"], evidence: null, context: null }}
      />
    );
    expect(screen.getByTestId("review-digest")).toBeTruthy();
    expect(screen.queryByTestId("review-plan")).toBeNull();
    expect(screen.getByTestId("review-artifacts")).toHaveTextContent("only.ts");
  });

  it("v2.9 运行仪表：仅 context 也渲染摘要区——五段占比条 + 图例 + 累计成本行", () => {
    render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        digest={{
          plan: null,
          subtasks: [],
          artifacts: [],
          evidence: null,
          context: {
            breakdown: { system: 500, l1: 100, history: 300, l5: 50, tools: 50 },
            estTotal: 1000,
            totals: { inputTokens: 1200, outputTokens: 40, costUsd: 0.0056 },
          },
        }}
      />
    );
    expect(screen.getByTestId("review-digest")).toBeTruthy();
    const segs = screen.getByTestId("review-ctx-bar").querySelectorAll(".review-ctx-seg");
    expect(segs).toHaveLength(5);
    expect(segs[0].getAttribute("style")).toContain("50%");
    expect(segs[2].getAttribute("style")).toContain("30%");
    const legend = screen.getByTestId("review-ctx-legend");
    expect(legend.textContent).toContain("review.ctx_system");
    expect(legend.textContent).toContain("500");
    const cost = screen.getByTestId("review-ctx-cost");
    expect(cost.textContent).toContain("1200");
    expect(cost.textContent).toContain("0.0056");
    expect(screen.queryByTestId("review-plan")).toBeNull();
    expect(screen.queryByTestId("review-evidence")).toBeNull();
  });
});
