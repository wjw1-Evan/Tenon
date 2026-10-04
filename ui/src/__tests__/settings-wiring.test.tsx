// 设置接线回归：App 全链路 —— 命令面板打开设置面板（§7.2 接线验证）
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import App from "../App";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
  // fetch → 返回最小 settings（App boot 的 getSettings 走真实 fetch）
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
            approval_timeout_s: 120,
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

describe("app settings wiring probe", () => {
  it("命令面板 → Open settings → 面板渲染并回填", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    // 打开命令面板（Cmd/Ctrl+Shift+P）→ 出现设置命令
    fireEvent.keyDown(window, { key: "p", shiftKey: true, metaKey: true });
    // v1.62 起活动栏齿轮 aria-label 同名，面板内命令需限定作用域查询
    const palette = await screen.findByTestId("command-palette");
    fireEvent.click(within(palette).getByRole("button", { name: "Open settings" }));
    await waitFor(() =>
      expect(document.querySelector('[data-testid="settings-overlay"]')).toBeTruthy()
    );
    // 回填：boot 拉取的设置
    const buffer = document.getElementById("set-buffer") as HTMLInputElement;
    expect(buffer.value).toBe("2000");
    const mode = document.getElementById("set-mode") as HTMLSelectElement;
    expect(mode.value).toBe("interactive");
  });

  it("活动栏底部齿轮 → 面板渲染（v1.62 可见入口）", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    fireEvent.click(await screen.findByTestId("rail-settings"));
    await waitFor(() =>
      expect(document.querySelector('[data-testid="settings-overlay"]')).toBeTruthy()
    );
  });

  it("设置面板「编辑器」保存方式即时写 ui_prefs（§8.2 v1.75）", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    fireEvent.click(await screen.findByTestId("rail-settings"));
    await waitFor(() =>
      expect(document.querySelector('[data-testid="settings-overlay"]')).toBeTruthy()
    );
    const select = document.getElementById("set-save-mode") as HTMLSelectElement;
    expect(select.value).toBe("auto");
    fireEvent.change(select, { target: { value: "manual" } });
    const put = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
      ([url, init]) =>
        String(url).includes("/ui-prefs") &&
        (init as RequestInit | undefined)?.method === "PUT"
    );
    expect(put).toBeTruthy();
    expect(JSON.parse((put![1] as RequestInit).body as string)).toEqual({
      "editor.saveMode": "manual",
    });
  });

  it("命令面板包含编辑器撤销/重做（§8.2 v1.75）", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    fireEvent.keyDown(window, { key: "p", shiftKey: true, metaKey: true });
    const palette = await screen.findByTestId("command-palette");
    expect(within(palette).getByRole("button", { name: "Editor: Undo" })).toBeTruthy();
    expect(within(palette).getByRole("button", { name: "Editor: Redo" })).toBeTruthy();
  });
});
