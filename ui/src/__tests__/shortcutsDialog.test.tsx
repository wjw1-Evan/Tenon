// 快捷键速查表（§7.5 v1.167）：渲染分组 + Esc 收起 preventDefault（不穿透暂停代理）
// + 连接状态点三态渲染与点击重连。
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ShortcutsDialog } from "../components/ShortcutsDialog";
import { WsStatusDot } from "../components/WsStatusDot";
import { SHORTCUT_GROUPS, formatCombo } from "../lib/shortcuts";

const t = (key: string) => key;

describe("ShortcutsDialog", () => {
  it("renders all groups and entries from the single-source table", () => {
    render(<ShortcutsDialog open onClose={() => {}} t={t} />);
    for (const group of SHORTCUT_GROUPS) {
      expect(screen.getByText(group.titleKey)).toBeTruthy();
      for (const item of group.items) {
        expect(screen.getByText(item.labelKey)).toBeTruthy();
      }
    }
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("renders nothing when closed", () => {
    const { container } = render(<ShortcutsDialog open={false} onClose={() => {}} t={t} />);
    expect(container.firstChild).toBeNull();
  });

  it("Esc closes and preventDefaults (must not leak into global pause)", () => {
    const onClose = vi.fn();
    render(<ShortcutsDialog open onClose={onClose} t={t} />);
    const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true });
    const spy = vi.spyOn(event, "preventDefault");
    window.dispatchEvent(event);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalled();
  });
});

describe("formatCombo", () => {
  it("normalizes spec keys to platform display", () => {
    const isMac = /Mac|iPhone|iPad/.test(navigator.userAgent);
    const palette = formatCombo("mod+shift+p");
    expect(palette.toLowerCase()).toContain("p");
    expect(palette).toContain(isMac ? "⌘" : "Ctrl");
    expect(formatCombo("escape").toLowerCase()).toContain("esc");
    const comma = formatCombo("mod+,");
    expect(comma).toContain(isMac ? "⌘" : "Ctrl");
    expect(comma).toContain(",");
  });
});

describe("WsStatusDot", () => {
  it("renders three states with localized labels and fires reconnect on click", () => {
    const onReconnect = vi.fn();
    const { rerender } = render(
      <WsStatusDot status="online" t={t} onReconnect={onReconnect} />
    );
    const dot = screen.getByTestId("ws-status");
    expect(dot.getAttribute("data-status")).toBe("online");
    expect(dot.getAttribute("aria-label")).toBe("topbar.ws_online");
    fireEvent.click(dot);
    expect(onReconnect).toHaveBeenCalledTimes(1);

    rerender(<WsStatusDot status="connecting" t={t} onReconnect={onReconnect} />);
    expect(screen.getByTestId("ws-status").getAttribute("data-status")).toBe("connecting");
    rerender(<WsStatusDot status="offline" t={t} onReconnect={onReconnect} />);
    expect(screen.getByTestId("ws-status").getAttribute("data-status")).toBe("offline");
  });
});
