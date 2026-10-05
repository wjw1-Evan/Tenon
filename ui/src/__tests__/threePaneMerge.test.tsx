// 三栏合并 UI 测试：三选一交互 / 冲突降级 / resolve 回调。
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ThreePaneMerge } from "../components/ThreePaneMerge";
import type { DirtyConflict } from "../components/ThreePaneMerge";

afterEach(cleanup);

const labels = {
  ours: "我的改动",
  theirs: "代理改动",
  base: "合并结果",
  apply: "应用合并",
  keepMine: "保留我的",
  useAgent: "应用代理",
};

function makeConflict(overrides?: Partial<DirtyConflict>): DirtyConflict {
  return {
    path: "src/app.ts",
    base: "const a = 1;\nconst b = 2;\n",
    ours: "const a = 100;\nconst b = 2;\n",
    theirs: "const a = 1;\nconst b = 200;\n",
    ...overrides,
  };
}

describe("ThreePaneMerge", () => {
  it("renders three panes with merge result selected by default", () => {
    render(<ThreePaneMerge conflict={makeConflict()} onResolve={vi.fn()} labels={labels} />);
    expect(screen.getByTestId("three-pane-merge")).toBeTruthy();
    expect(screen.getByTestId("pane-theirs")).toHaveTextContent("const b = 200");
    expect(screen.getByTestId("pane-ours")).toHaveTextContent("const a = 100");
    // 三方合并没有冲突 → merged pane 有内容
    expect(screen.getByTestId("pane-merged").textContent).toContain("const a = 100");
  });

  it("applies the selected pane content on resolve", () => {
    const onResolve = vi.fn();
    render(<ThreePaneMerge conflict={makeConflict()} onResolve={onResolve} labels={labels} />);
    fireEvent.click(screen.getByTestId("merge-apply"));
    // 默认选 merged → 应用合并结果
    expect(onResolve).toHaveBeenCalledWith("src/app.ts", expect.stringContaining("const a = 100"), true);
  });

  it("switches to ours pane and applies", () => {
    const onResolve = vi.fn();
    render(<ThreePaneMerge conflict={makeConflict()} onResolve={onResolve} labels={labels} />);
    // 点选"我的改动"
    const oursRadio = screen.getAllByRole("radio")[1];
    fireEvent.click(oursRadio);
    fireEvent.click(screen.getByTestId("merge-apply"));
    expect(onResolve).toHaveBeenCalledWith("src/app.ts", expect.stringContaining("const a = 100"), true);
  });

  it("keepMine resolves with theirs content and clearBuffer=false", () => {
    const onResolve = vi.fn();
    render(<ThreePaneMerge conflict={makeConflict()} onResolve={onResolve} labels={labels} />);
    fireEvent.click(screen.getByTestId("merge-keep-mine"));
    expect(onResolve).toHaveBeenCalledWith("src/app.ts", expect.stringContaining("const b = 200"), false);
  });

  it("useAgent resolves with ours content", () => {
    const onResolve = vi.fn();
    render(<ThreePaneMerge conflict={makeConflict()} onResolve={onResolve} labels={labels} />);
    fireEvent.click(screen.getByTestId("merge-use-agent"));
    expect(onResolve).toHaveBeenCalledWith("src/app.ts", expect.stringContaining("const a = 100"), true);
  });

  it("shows conflict hint and disables merged pane for overlapping edits", () => {
    const conflict = makeConflict({
      base: "same\nsame\n",
      ours: "ours1\nours2\n",
      theirs: "theirs1\ntheirs2\n",
    });
    render(<ThreePaneMerge conflict={conflict} onResolve={vi.fn()} labels={labels} />);
    // 两侧同区改动 → 提示手动选择 + merged 禁用
    expect(screen.getByText(/请手动选择/)).toBeTruthy();
    const mergedRadio = screen.getAllByRole("radio")[2] as HTMLInputElement;
    expect(mergedRadio.disabled).toBe(true);
    expect(screen.getByTestId("pane-merged")).toHaveTextContent("—");
  });
});
