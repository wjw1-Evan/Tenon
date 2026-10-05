import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DiffPanel, diffFromPatchEvent, parseDiffLines } from "../components/DiffPanel";

describe("diff 面板（M0 交付）", () => {
  it("渲染统一 diff 内容", () => {
    const diff = "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-old\n+new";
    render(<DiffPanel diff={diff} />);
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("+new");
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("-old");
  });

  it("无改动时显示占位（默认英文源语言）", () => {
    render(<DiffPanel diff={null} />);
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("(no changes)");
  });

  it("从 patch_applied 事件提取 diff", () => {
    const payload = {
      tool: "apply_patch",
      output: { ok: true, content: "已写入 f.txt\n--- a/f.txt\n+++ b/f.txt\n+x" },
    };
    expect(diffFromPatchEvent(payload)).toContain("+x");
    // 非 patch 事件 / 无 diff 内容返回 null
    expect(diffFromPatchEvent({ tool: "grep", output: { ok: true, content: "hits" } })).toBeNull();
    expect(diffFromPatchEvent({})).toBeNull();
  });

  it("万行 diff 只物化可视窗口并支持滚动到尾部", async () => {
    const diff = [
      "--- a/large.txt",
      "+++ b/large.txt",
      ...Array.from({ length: 20_000 }, (_, index) => `+line-${index}`),
    ].join("\n");
    render(<DiffPanel diff={diff} viewportHeight={240} />);
    const body = screen.getByTestId("diff-body");
    const initialRows = screen.getAllByTestId("diff-line");
    expect(initialRows.length).toBeLessThanOrEqual(80);
    expect(body).toHaveTextContent("line-8");
    expect(body).not.toHaveTextContent("line-19999");

    // jsdom 不做布局/滚动截断；将 readonly scrollTop 模拟到接近尾部的位置。
    Object.defineProperty(body, "scrollTop", {
      configurable: true,
      value: 19_990 * 18,
    });
    fireEvent.scroll(body);
    await waitFor(() => expect(screen.getByText("+line-19999")).toBeTruthy());
    expect(screen.getAllByTestId("diff-line").length).toBeLessThanOrEqual(80);
  });

  it("diff 解析保持线性输出且 20k 行低于单帧预算", () => {
    const diff = Array.from({ length: 20_000 }, (_, i) => `+line-${i}`).join("\n");
    const started = performance.now();
    const parsed = parseDiffLines(diff);
    expect(parsed).toHaveLength(20_000);
    expect(parsed[19_999]).toBe("+line-19999");
    expect(performance.now() - started).toBeLessThan(16);
  });
});

describe("DiffPanel 补充", () => {
  it("parseDiffLines 分类行类型", () => {
    const lines = parseDiffLines("--- a/f\n+++ b/f\n@@ -1 +1 @@\n context\n-old\n+new\n");
    expect(lines.length).toBeGreaterThan(0);
  });

  it("hunk 头渲染", () => {
    const diff = "--- a/f.txt\n+++ b/f.txt\n@@ -3,4 +3,4 @@\n context\n";
    render(<DiffPanel diff={diff} />);
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("@@");
  });

  it("empty diff shows caller-localized placeholder", () => {
    render(<DiffPanel diff="" emptyText="（无改动）" />);
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("（无改动）");
  });
});
