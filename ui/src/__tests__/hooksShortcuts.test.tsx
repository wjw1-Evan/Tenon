// hooks.ts 快捷键全覆盖测试（含 v1.167 扩展：Alt 档新任务 / 关标签 / 循环底部 tab / 速查表）。
import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useShortcuts } from "../hooks";

function fireKey(key: string, opts: Partial<KeyboardEventInit> = {}) {
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...opts }));
}

describe("useShortcuts", () => {
  it("all shortcuts trigger their handlers", () => {
    const handlers = {
      onPalette: vi.fn(),
      onGotoFile: vi.fn(),
      onInlineInstruction: vi.fn(),
      onPauseOrClose: vi.fn(),
      onStop: vi.fn(),
      onSidebar: vi.fn(),
      onPanel: vi.fn(),
      onSave: vi.fn(),
      onSettings: vi.fn(),
      onNewTask: vi.fn(),
      onCloseTab: vi.fn(),
      onCycleBottomTab: vi.fn(),
      onShortcuts: vi.fn(),
    };
    renderHook(() => useShortcuts(handlers));

    fireKey("p", { metaKey: true, shiftKey: true });
    expect(handlers.onPalette).toHaveBeenCalledTimes(1);
    fireKey("p", { metaKey: true });
    expect(handlers.onGotoFile).toHaveBeenCalledTimes(1);
    fireKey("i", { metaKey: true });
    expect(handlers.onInlineInstruction).toHaveBeenCalledTimes(1);
    fireKey("Escape");
    expect(handlers.onPauseOrClose).toHaveBeenCalledTimes(1);
    fireKey(".", { metaKey: true });
    expect(handlers.onStop).toHaveBeenCalledTimes(1);
    fireKey("b", { metaKey: true });
    expect(handlers.onSidebar).toHaveBeenCalledTimes(1);
    fireKey("j", { metaKey: true });
    expect(handlers.onPanel).toHaveBeenCalledTimes(1);
    fireKey("s", { metaKey: true });
    expect(handlers.onSave).toHaveBeenCalledTimes(1);
    fireKey(",", { metaKey: true });
    expect(handlers.onSettings).toHaveBeenCalledTimes(1);
    fireKey("n", { metaKey: true, altKey: true });
    expect(handlers.onNewTask).toHaveBeenCalledTimes(1);
    fireKey("w", { metaKey: true, altKey: true });
    expect(handlers.onCloseTab).toHaveBeenCalledTimes(1);
    fireKey("j", { metaKey: true, altKey: true });
    expect(handlers.onCycleBottomTab).toHaveBeenCalledTimes(1);
    fireKey("/", { metaKey: true });
    expect(handlers.onShortcuts).toHaveBeenCalledTimes(1);
  });

  it("alt variants do not leak into plain handlers (mod+alt+j ≠ panel toggle)", () => {
    const handlers = { onPanel: vi.fn(), onCycleBottomTab: vi.fn(), onNewTask: vi.fn() };
    renderHook(() => useShortcuts(handlers));
    fireKey("j", { metaKey: true, altKey: true });
    expect(handlers.onCycleBottomTab).toHaveBeenCalledTimes(1);
    expect(handlers.onPanel).not.toHaveBeenCalled();
    // 无 Alt 的 n/w 不触发（防误触）
    fireKey("n", { metaKey: true });
    fireKey("w", { metaKey: true });
    expect(handlers.onNewTask).not.toHaveBeenCalled();
  });

  it("does not trigger without modifier for mod-only shortcuts", () => {
    const handlers = { onSave: vi.fn(), onStop: vi.fn() };
    renderHook(() => useShortcuts(handlers));
    fireKey("s");
    fireKey(".");
    expect(handlers.onSave).not.toHaveBeenCalled();
    expect(handlers.onStop).not.toHaveBeenCalled();
  });

  it("cleans up listeners on unmount", () => {
    const handlers = { onPalette: vi.fn() };
    const { unmount } = renderHook(() => useShortcuts(handlers));
    unmount();
    fireKey("p", { metaKey: true, shiftKey: true });
    expect(handlers.onPalette).not.toHaveBeenCalled();
  });

  it("ctrl key works as modifier (cross-platform)", () => {
    const handlers = { onSave: vi.fn() };
    renderHook(() => useShortcuts(handlers));
    fireKey("s", { ctrlKey: true });
    expect(handlers.onSave).toHaveBeenCalledTimes(1);
  });
});
