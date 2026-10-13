// 终端面板组件测试（§7.2 v1.197 / v2.42 握手修复 / v2.45 测试补强）：
// xterm mock + 假 WS 桩——成功链路（开终端→连 WS→resize 帧→输出/输入接线）、
// 失败链路（openTerminal 拒绝 → open_failed 文案）、卸载清理（断流不杀进程语义
// 下 socket 关闭 + term 释放）、无项目不请求。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { TerminalPanel } from "../components/TerminalPanel";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

// xterm.js 依赖 canvas/DOM 测量，jsdom 跑不了真实例——同 Monaco 全局 mock 的
// 惯例，在文件级替换 Terminal / FitAddon，并以实例数组暴露断言面。
const { FakeTerminal, terminals } = vi.hoisted(() => {
  class FakeTerminal {
    static instances: FakeTerminal[] = [];
    written: string[] = [];
    writelnCalls: string[] = [];
    dataHandlers: ((d: string) => void)[] = [];
    disposed = false;
    focused = 0;
    rows = 24;
    cols = 80;
    constructor(_opts?: unknown) {
      FakeTerminal.instances.push(this);
    }
    loadAddon() {}
    open() {}
    onData(cb: (d: string) => void) {
      this.dataHandlers.push(cb);
      return { dispose() {} };
    }
    write(d: string) {
      this.written.push(d);
    }
    writeln(d: string) {
      this.writelnCalls.push(d);
    }
    focus() {
      this.focused++;
    }
    dispose() {
      this.disposed = true;
    }
  }
  return { FakeTerminal, terminals: FakeTerminal.instances };
});

vi.mock("@xterm/xterm", () => ({ Terminal: FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));

beforeEach(() => {
  // jsdom 无 ResizeObserver（面板成功链路用于尺寸同步）
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
  );
});

afterEach(() => {
  terminals.length = 0;
  vi.unstubAllGlobals();
});

const t: Translate = (key) => (key === "terminal.open_failed" ? "TERMINAL_FAILED" : key);

type Captured = {
  onOutput?: (data: string | Uint8Array) => void;
  onClose?: () => void;
};

function makeApi(opts?: { failOpen?: boolean }) {
  class FakeSocket {
    sent: string[] = [];
    closed = false;
    // 面板以 ws.readyState === WebSocket.OPEN（=1）守卫发送
    readyState = 1;
    send(d: string) {
      this.sent.push(d);
    }
    close() {
      this.closed = true;
    }
  }
  const sockets: FakeSocket[] = [];
  const captured: Captured = {};
  const raw = {
    openTerminal: vi.fn().mockImplementation(async () => {
      if (opts?.failOpen) throw new Error("daemon exploded");
      return { shell: "/bin/zsh" };
    }),
    connectTerminal: vi.fn().mockImplementation(
      async (
        _pid: string,
        onOutput: (data: string | Uint8Array) => void,
        onClose: () => void
      ) => {
        captured.onOutput = onOutput;
        captured.onClose = onClose;
        const s = new FakeSocket();
        sockets.push(s);
        return s as unknown as WebSocket;
      }
    ),
    closeTerminal: vi.fn().mockResolvedValue(undefined),
  };
  return { api: raw as unknown as TenonApi, sockets, captured, mocks: raw };
}

describe("TerminalPanel（§7.2 项目终端）", () => {
  it("成功链路：开终端→连 WS→发 resize 控制帧→输出/输入双向接线", async () => {
    const { api, sockets, captured } = makeApi();
    const { unmount } = render(
      <TerminalPanel api={api} projectId="p1" t={t} />
    );

    await waitFor(() => expect(terminals.length).toBe(1));
    expect(screen.getByTestId("terminal-panel")).toBeTruthy();
    expect(api.openTerminal).toHaveBeenCalledWith("p1");
    expect(api.connectTerminal).toHaveBeenCalledWith(
      "p1",
      expect.any(Function),
      expect.any(Function)
    );

    // 连接后即发尺寸同步帧（fit 初始 24x80）
    await waitFor(() => expect(sockets[0]?.sent.length).toBeGreaterThan(0));
    expect(JSON.parse(sockets[0].sent[0])).toEqual({ resize: [24, 80] });

    // daemon 输出 → term.write；用户键入 → ws.send
    captured.onOutput?.("hello from pty");
    expect(terminals[0].written).toContain("hello from pty");
    terminals[0].dataHandlers[0]("ls\r");
    expect(sockets[0].sent).toContain("ls\r");

    unmount();
  });

  it("打开失败：openTerminal 拒绝 → 终端内写 open_failed 文案", async () => {
    const { api } = makeApi({ failOpen: true });
    render(<TerminalPanel api={api} projectId="p1" t={t} />);

    await waitFor(() => expect(terminals.length).toBe(1));
    await waitFor(() =>
      expect(terminals[0].writelnCalls).toContain("TERMINAL_FAILED")
    );
    expect(api.connectTerminal).not.toHaveBeenCalled();
  });

  it("卸载清理：断开 WS、释放 term（关 tab 仅断流不杀进程）", async () => {
    const { api, sockets } = makeApi();
    const { unmount } = render(
      <TerminalPanel api={api} projectId="p1" t={t} />
    );

    await waitFor(() => expect(sockets.length).toBe(1));
    unmount();
    expect(sockets[0].closed).toBe(true);
    expect(terminals[0].disposed).toBe(true);
  });

  it("无项目：不实例化终端、不发任何请求", () => {
    const { api } = makeApi();
    render(<TerminalPanel api={api} projectId={null} t={t} />);
    expect(terminals.length).toBe(0);
    expect(api.openTerminal).not.toHaveBeenCalled();
    expect(api.connectTerminal).not.toHaveBeenCalled();
  });
});
