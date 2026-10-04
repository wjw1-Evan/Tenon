import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useShortcuts } from "../hooks";

function Probe({ handlers }: { handlers: Parameters<typeof useShortcuts>[0] }) {
  useShortcuts(handlers);
  return null;
}

describe("快捷键（§7.4 核心集）", () => {
  it("Cmd+Shift+P → 命令面板；Cmd+P → 模糊打开；Cmd+. → 停止；Cmd+Alt+Z → 时间轴；Esc → 暂停", () => {
    const handlers = {
      onPalette: vi.fn(),
      onGotoFile: vi.fn(),
      onStop: vi.fn(),
      onTimeline: vi.fn(),
      onPauseOrClose: vi.fn(),
    };
    render(<Probe handlers={handlers} />);
    const press = (init: KeyboardEventInit) =>
      window.dispatchEvent(new KeyboardEvent("keydown", init));

    press({ key: "p", metaKey: true, shiftKey: true, bubbles: true });
    expect(handlers.onPalette).toHaveBeenCalledTimes(1);
    press({ key: "p", metaKey: true, bubbles: true });
    expect(handlers.onGotoFile).toHaveBeenCalledTimes(1);
    press({ key: ".", metaKey: true, bubbles: true });
    expect(handlers.onStop).toHaveBeenCalledTimes(1);
    press({ key: "z", metaKey: true, altKey: true, bubbles: true });
    expect(handlers.onTimeline).toHaveBeenCalledTimes(1);
    press({ key: "Escape", bubbles: true });
    expect(handlers.onPauseOrClose).toHaveBeenCalledTimes(1);
  });

  it("Cmd+I → 行内 AI 指令；Cmd+J → 底部面板；Cmd+B → 侧栏（v1.39）", () => {
    const handlers = {
      onInlineInstruction: vi.fn(),
      onPanel: vi.fn(),
      onSidebar: vi.fn(),
    };
    render(<Probe handlers={handlers} />);
    const press = (init: KeyboardEventInit) =>
      window.dispatchEvent(new KeyboardEvent("keydown", init));

    press({ key: "i", metaKey: true, bubbles: true });
    expect(handlers.onInlineInstruction).toHaveBeenCalledTimes(1);
    // Cmd+Shift+I / Cmd+Alt+I 不触发行内指令（避免占用系统/浏览器组合键语义）
    press({ key: "i", metaKey: true, shiftKey: true, bubbles: true });
    press({ key: "i", metaKey: true, altKey: true, bubbles: true });
    expect(handlers.onInlineInstruction).toHaveBeenCalledTimes(1);
    press({ key: "j", metaKey: true, bubbles: true });
    expect(handlers.onPanel).toHaveBeenCalledTimes(1);
    press({ key: "b", metaKey: true, bubbles: true });
    expect(handlers.onSidebar).toHaveBeenCalledTimes(1);
  });
});
