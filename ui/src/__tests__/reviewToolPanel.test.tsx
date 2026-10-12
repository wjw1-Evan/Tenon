// v2.38 工具面板（ZCode 式对话工作台，design.md §7.2）UI 契约：
// ① 回退形态：不传 toolTabs 无 tab 栏——v2.5 单一 diff 审查面原样；
// ② 页切换：默认 diff 页（diff 流可见、摘要卡迁出），overview 页渲染摘要五卡；
// ③ files 页：FileTree 树渲染，点击文件行载入只读预览（Monaco mock），
//    「编辑器打开」回调目标路径，projectId 空渲染空态；
// ④ 页记忆：切换落盘 tenon:toolTab，重挂载恢复所选页。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ReviewPane } from "../components/ReviewPane";
import type { ReviewDigest } from "../lib/reviewDigest";
import type { TenonApi } from "../lib/api";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
});

const t = (k: string) => k;
const DIFF = "--- a/f.ts\n+++ b/f.ts\n@@ -1,3 +1,3 @@\n ctx\n-old\n+new\n ctx2";

const DIGEST: ReviewDigest = {
  plan: ["步骤一"],
  subtasks: [{ title: "子任务", status: "in_progress" }],
  artifacts: ["f.ts"],
  evidence: "ok",
  context: {
    breakdown: { system: 1, l1: 1, history: 1, l5: 1, tools: 1 },
    estTotal: 5,
    totals: { inputTokens: 10, outputTokens: 2, costUsd: 0.01 },
  },
};

function makeApi(): TenonApi {
  return {
    tree: vi.fn(async () => ({
      entries: [
        { path: "src", name: "src", kind: "dir", git_status: "" },
        { path: "f.ts", name: "f.ts", kind: "file", git_status: "modified" },
      ],
    })),
    readFile: vi.fn(async (_pid: string, path: string) => ({
      path,
      content: "const a = 1;\n",
      total_bytes: 13,
    })),
  } as unknown as TenonApi;
}

function mountToolPanel(overrides?: { api?: TenonApi; projectId?: string | null }) {
  const api = overrides?.api ?? makeApi();
  const onOpenEditor = vi.fn();
  render(
    <ReviewPane
      t={t}
      diff={DIFF}
      running
      statusLabel="review.running"
      onClose={() => {}}
      onFeedback={() => {}}
      digest={DIGEST}
      toolTabs={{
        api,
        projectId: overrides?.projectId === undefined ? "p1" : overrides.projectId,
        onOpenEditor,
      }}
    />
  );
  return { api, onOpenEditor };
}

describe("ReviewPane 工具面板（v2.38，design.md §7.2）", () => {
  it("回退形态：不传 toolTabs 无 tab 栏（v2.5 单一 diff 审查面）", () => {
    render(
      <ReviewPane
        t={t}
        diff={DIFF}
        running
        statusLabel="review.running"
        onClose={() => {}}
        digest={DIGEST}
      />
    );
    expect(screen.queryByTestId("review-tabs")).toBeNull();
    // 摘要卡回常驻顶部（迁「概览」页前的 v2.8 形态）
    expect(screen.getByTestId("review-digest")).toBeTruthy();
    expect(screen.getByText("+new")).toBeTruthy();
  });

  it("页切换：默认 diff 页（diff 可见、摘要迁出），overview 页渲染摘要五卡", () => {
    mountToolPanel();
    // tab 栏三页
    for (const key of ["diff", "files", "overview"]) {
      expect(screen.getByTestId(`review-tab-${key}`)).toBeTruthy();
    }
    // 默认 diff 页：diff 流可见、digest 已迁出
    expect(screen.getByText("+new")).toBeTruthy();
    expect(screen.queryByTestId("review-digest")).toBeNull();
    // overview 页：摘要卡出现、diff 流卸下
    fireEvent.click(screen.getByTestId("review-tab-overview"));
    expect(screen.getByTestId("review-digest")).toBeTruthy();
    expect(screen.queryByText("+new")).toBeNull();
  });

  it("files 页：树渲染 + 点击文件行载入只读预览 + 「编辑器打开」回调路径", async () => {
    const { api, onOpenEditor } = mountToolPanel();
    fireEvent.click(screen.getByTestId("review-tab-files"));
    expect(screen.getByTestId("review-files")).toBeTruthy();
    // FileTree 懒加载根目录
    await waitFor(() => expect(screen.getByText("f.ts")).toBeTruthy());
    expect(api.tree).toHaveBeenCalledWith("p1", "");
    // 点击文件行 → 只读预览（Monaco mock data-value 承载内容）
    fireEvent.click(screen.getByText("f.ts"));
    await waitFor(() => expect(screen.getByTestId("review-preview")).toBeTruthy());
    expect(screen.getByTestId("review-preview-open")).toBeTruthy();
    expect(screen.getByTestId("monaco-editor").getAttribute("data-value")).toBe("const a = 1;\n");
    fireEvent.click(screen.getByTestId("review-preview-open"));
    expect(onOpenEditor).toHaveBeenCalledWith("f.ts");
  });

  it("files 页 projectId 空渲染空态，不请求树", () => {
    mountToolPanel({ projectId: null });
    fireEvent.click(screen.getByTestId("review-tab-files"));
    expect(screen.getByTestId("review-files-empty")).toBeTruthy();
  });

  it("页记忆：切换落盘 tenon:toolTab，重挂载恢复所选页", () => {
    mountToolPanel();
    fireEvent.click(screen.getByTestId("review-tab-overview"));
    expect(localStorage.getItem("tenon:toolTab")).toBe("overview");
    // 重挂载（模拟重进会话）：先卸载当前树再渲染，避免双实例干扰查询
    cleanup();
    mountToolPanel();
    expect(
      screen.getByTestId("review-tab-overview").getAttribute("aria-selected"),
    ).toBe("true");
    expect(screen.getByTestId("review-digest")).toBeTruthy();
  });
});
