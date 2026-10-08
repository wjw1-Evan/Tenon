// 全局通知（§7.5 v1.167）：store 行为契约 + ToastHost 渲染。
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ToastHost } from "../components/ToastHost";
import {
  __resetToastsForTest,
  dismiss,
  getToasts,
  toast,
} from "../lib/toast";

beforeEach(() => {
  __resetToastsForTest();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("toast store", () => {
  it("success/info auto-dismiss after 3s; error persists until manually dismissed", () => {
    toast.success("ok");
    toast.error("bad");
    expect(getToasts().map((t) => t.kind)).toEqual(["success", "error"]);
    act(() => {
      vi.advanceTimersByTime(3100);
    });
    const remaining = getToasts();
    expect(remaining.map((t) => t.kind)).toEqual(["error"]);
    act(() => {
      dismiss(remaining[0].id);
    });
    expect(getToasts()).toEqual([]);
  });

  it("same error message is deduped within 5s window", () => {
    toast.error("保存失败");
    toast.error("保存失败");
    expect(getToasts()).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(5100);
    });
    toast.error("保存失败");
    expect(getToasts()).toHaveLength(2);
  });

  it("stack caps at 3, evicting oldest non-error first", () => {
    toast.info("a");
    toast.info("b");
    toast.error("e1");
    toast.info("c");
    const items = getToasts();
    expect(items).toHaveLength(3);
    expect(items.map((t) => t.message)).toEqual(["b", "e1", "c"]);
    // 全为 error 时挤最旧 error
    toast.error("e2");
    toast.error("e3");
    const after = getToasts();
    expect(after).toHaveLength(3);
    expect(after.every((t) => t.kind === "error")).toBe(true);
    expect(after.map((t) => t.message)).toEqual(["e1", "e2", "e3"]);
  });

  it("blank messages are ignored", () => {
    toast.error("   ");
    expect(getToasts()).toHaveLength(0);
  });
});

describe("ToastHost", () => {
  it("renders error with alert role and close button; status for auto-dismiss kinds", () => {
    toast.error("boom");
    toast.info("hello");
    render(<ToastHost t={(key) => key} />);
    expect(screen.getByRole("alert").textContent).toContain("boom");
    expect(screen.getByRole("status").textContent).toContain("hello");

    const errorId = getToasts().find((t) => t.kind === "error")!.id;
    fireEvent.click(screen.getByTestId(`toast-close-${errorId}`));
    expect(getToasts().some((t) => t.id === errorId)).toBe(false);
  });

  it("renders nothing when empty", () => {
    const { container } = render(<ToastHost t={(key) => key} />);
    expect(container.firstChild).toBeNull();
  });
});
