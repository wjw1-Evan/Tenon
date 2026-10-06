// 侧栏源码区动线（§7.2 v1.139）：上下拆分侧栏——任务流恒在、源码树驻下部；
// 文件 / 文件夹直接拖入任务输入框插 @路径；点文件经 App 弹出编辑器浮层。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import App from "../App";

function stubRepoFetch() {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockImplementation((input: RequestInfo | URL) => {
      const url = String(input);
      let body: unknown = {};
      if (url.includes("/pairing")) {
        body = { port: 9876, token: "dev" };
      } else if (url.includes("/ws-ticket")) {
        body = { ticket: "t", expires_in_s: 60 };
      } else if (url.includes("/projects/open")) {
        body = { id: "p1", path: "/tmp/repo", display_name: "repo", trusted: true };
      } else if (url.includes("/projects")) {
        body = {
          projects: [
            {
              id: "p1",
              path: "/tmp/repo",
              display_name: "repo",
              trusted: true,
              sessions: [
                {
                  id: "s1",
                  status: "done",
                  model: "mock",
                  title: "Fix login bug",
                  updated_at: "2026-10-06T00:00:00Z",
                },
              ],
              active_sessions: 0,
              dirty_buffers: 0,
              usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
            },
          ],
        };
      } else if (url.includes("/ui-state")) {
        body = {};
      } else if (url.includes("/file?")) {
        body = { path: "a.txt", content: "hello", total_bytes: 5 };
      } else if (url.includes("/tree")) {
        body = {
          entries: [
            { path: "src", name: "src", kind: "dir", git_status: "" },
            { path: "a.txt", name: "a.txt", kind: "file", git_status: "" },
          ],
        };
      } else if (url.includes("/session")) {
        body = { session_id: "s1", project_id: "p1" };
      }
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
    })
  );
}

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
  vi.stubGlobal("innerWidth", 1280);
  vi.stubGlobal("innerHeight", 800);
  vi.stubGlobal("confirm", () => true);
  stubRepoFetch();
});

afterEach(() => vi.unstubAllGlobals());

describe("侧栏源码区动线（§7.2 v1.139）", () => {
  it("点「源码」拆分侧栏：下区文件树出现，线程主区与任务流不受影响", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    fireEvent.click(screen.getByTestId("pe-source-p1"));
    await waitFor(() => expect(screen.getByTestId("pe-source-pane")).toBeTruthy());
    // 上区任务流与主区线程均不受影响（对话随时可用）。
    expect(screen.getByTestId("chat-list-p1")).toBeTruthy();
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
    expect(document.querySelector<HTMLElement>(".zone-thread")?.style.display).toBe("");
    // 会话级不落盘（v1.137 的 tenon:peView 已撤）。
    expect(localStorage.getItem("tenon:peView")).toBeNull();

    // ✕ 收起源码区：侧栏回整栏任务流。
    fireEvent.click(screen.getByTestId("pe-source-close"));
    await waitFor(() => expect(screen.queryByTestId("pe-source-pane")).toBeNull());
    expect(screen.getByTestId("chat-list-p1")).toBeTruthy();
  });

  it("点源码区文件行弹出编辑器浮层（✕ 关闭回对话）", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    fireEvent.click(screen.getByTestId("pe-source-p1"));
    await waitFor(() => expect(screen.getByTestId("pe-source-pane")).toBeTruthy());
    fireEvent.click(within(screen.getByTestId("pe-source-pane")).getByText("a.txt"));
    await waitFor(() => expect(screen.getByTestId("editor-overlay")).toBeTruthy());
    expect(screen.getByTestId("editor-pane")).toBeTruthy();
    fireEvent.click(screen.getByTestId("editor-overlay-close"));
    await waitFor(() => expect(screen.queryByTestId("editor-overlay")).toBeNull());
    // 源码区仍在，线程原样。
    expect(screen.getByTestId("pe-source-pane")).toBeTruthy();
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
  });

  it("源码区文件 / 文件夹直接拖入任务输入框插 @路径（同屏，不发送）", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    fireEvent.click(screen.getByTestId("pe-source-p1"));
    await waitFor(() => expect(screen.getByTestId("pe-source-pane")).toBeTruthy());
    // jsdom 不完整实现 DataTransfer（同 fileTreeDrag 桩法）：文件行拖入输入框。
    const inputBox = screen.getByTestId("task-input-box");
    fireEvent.drop(inputBox, {
      dataTransfer: {
        getData: (type: string) => (type === "application/x-tenon-path" ? "a.txt" : ""),
      },
    });
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    await waitFor(() => expect(input.value).toBe("@a.txt "));
    // 文件夹行同 MIME：拖入插 @目录。
    fireEvent.drop(inputBox, {
      dataTransfer: {
        getData: (type: string) => (type === "application/x-tenon-path" ? "src" : ""),
      },
    });
    await waitFor(() => expect(input.value).toBe("@a.txt @src "));
    // 不自动发送：输入仍在框内。
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
  });
});
