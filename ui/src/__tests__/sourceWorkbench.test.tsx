// 源码工作台动线闭环（§7.2 v1.138）：弹出层宿主——✕ 收回对话；拖入路由收层 + @路径追加。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
          entries: [{ path: "a.txt", name: "a.txt", kind: "file", git_status: "" }],
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

describe("源码工作台动线（§7.2 v1.138）", () => {
  it("弹出工作台时线程保持可见，✕ 收回对话", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    fireEvent.click(screen.getByTestId("pe-source-p1"));
    await waitFor(() => expect(screen.getByTestId("source-workbench")).toBeTruthy());
    // 线程主区不被替换：任务输入框仍在（对话随时可用）。
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
    expect(document.querySelector<HTMLElement>(".zone-thread")?.style.display).toBe("");
    fireEvent.click(screen.getByTestId("source-workbench-close"));
    await waitFor(() => expect(screen.queryByTestId("source-workbench")).toBeNull());
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
  });

  it("弹出层内拖入文件收层回对话并把 @路径 追加进输入框（不发送）", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    fireEvent.click(screen.getByTestId("pe-source-p1"));
    await waitFor(() => expect(screen.getByTestId("source-workbench")).toBeTruthy());
    // jsdom 不完整实现 DataTransfer（同 fileTreeDrag 桩法）：弹层非文件树行落下 x-tenon-path
    //（落在编辑器窗格，冒泡到 source-overlay-body 的 onDrop）。
    fireEvent.drop(screen.getByTestId("source-workbench-body"), {
      dataTransfer: {
        getData: (type: string) => (type === "application/x-tenon-path" ? "a.txt" : ""),
      },
    });
    await waitFor(() => expect(screen.queryByTestId("source-workbench")).toBeNull());
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    await waitFor(() => expect(input.value).toBe("@a.txt "));
    // 不自动发送：输入仍在框内，等用户补全任务语义。
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
  });
});
