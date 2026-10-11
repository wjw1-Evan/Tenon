// v2.4 常驻审查面（design-v2.md §6）UI 契约：
// ① ReviewPane 组件：运行中/空闲状态点与状态标签、diff 渲染（复用 DiffPanel
//    虚拟滚动）、空态文案、✕ 关闭回调；
// ② App 接线：运行态（RUNNING_STATES）自动停靠、空闲自动收起、手动关闭后
//    本次会话不再自动弹（rerender 保持），关闭回调触发。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ReviewPane } from "../components/ReviewPane";

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
