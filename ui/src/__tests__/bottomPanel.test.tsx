// 底部面板可见开合（v1.61 / 默认收起 v1.78）：展开态 tabs 行右端收起按钮、收起态细条恢复；Cmd+J / 命令面板等效
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
  vi.stubGlobal(
    "fetch",
    vi.fn().mockImplementation((input: RequestInfo | URL) => {
      const url = String(input);
      let body: unknown = {};
      if (url.includes("/pairing")) {
        body = { port: 9876, token: "dev" };
      } else if (url.includes("/projects") && !url.includes("open")) {
        body = { projects: [] };
      } else if (url.includes("/settings")) {
        body = {
          session: {
            mode: "interactive",
            first_edit_buffer_ms: 2000,

          },
          exec: { command_timeout_s: 120 },
        };
      } else if (url.includes("/portfolio-tasks")) {
        body = { tasks: [] };
      }
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
    })
  );
});

function renderApp() {
  return render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
}

describe("底部面板开合（§7.2 v1.78）", () => {
  it("默认收起：细条显示当前 tab 名，点击展开后可折叠", async () => {
    renderApp();
    await waitFor(() => expect(screen.getByTestId("bottom-open")).toBeTruthy());
    // v1.107：「源码」tab 迁右区源码树，底部枚举收敛为 时间轴 / 轨迹 / 评估。
    expect(screen.queryByTestId("tab-source")).toBeNull();

    fireEvent.click(screen.getByTestId("bottom-open"));
    expect(screen.getByTestId("tab-trace")).toBeTruthy();
    expect(screen.queryByTestId("tab-source")).toBeNull();
    expect(screen.queryByTestId("bottom-open")).toBeNull();

    fireEvent.click(screen.getByTestId("bottom-close"));
    const open = screen.getByTestId("bottom-open");
    expect(open.textContent).toContain("Conversation node timeline");
    expect(screen.queryByTestId("tab-source")).toBeNull();
  });

  it("Cmd+J 开合等效（v1.39 语义不变）", async () => {
    renderApp();
    await waitFor(() => expect(screen.getByTestId("bottom-open")).toBeTruthy());

    fireEvent.keyDown(window, { key: "j", metaKey: true });
    expect(screen.getByTestId("bottom-close")).toBeTruthy();
    expect(screen.queryByTestId("bottom-open")).toBeNull();

    fireEvent.keyDown(window, { key: "j", metaKey: true });
    expect(screen.queryByTestId("bottom-close")).toBeNull();
    expect(screen.getByTestId("bottom-open")).toBeTruthy();
  });

  it("命令面板 toggle.bottom 切换语义（对齐 toggle.sidebar）", async () => {
    renderApp();
    await waitFor(() => expect(screen.getByTestId("bottom-open")).toBeTruthy());

    fireEvent.keyDown(window, { key: "p", shiftKey: true, metaKey: true });
    const cmd = await screen.findByText("Toggle bottom panel");
    fireEvent.click(cmd);
    expect(screen.getByTestId("bottom-close")).toBeTruthy();
    expect(screen.queryByTestId("bottom-open")).toBeNull();

    fireEvent.keyDown(window, { key: "p", shiftKey: true, metaKey: true });
    fireEvent.click(await screen.findByText("Toggle bottom panel"));
    expect(screen.queryByTestId("bottom-close")).toBeNull();
    expect(screen.getByTestId("bottom-open")).toBeTruthy();
  });
});
