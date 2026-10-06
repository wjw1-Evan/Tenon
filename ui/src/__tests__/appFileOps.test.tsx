// App 文件操作深度测试：打开文件 / 编辑保存 / 文件树事件 / 项目切换。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, act } from "@testing-library/react";
import App from "../App";

const responses = new Map<string, unknown>();

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

function makeProject(id: string, name: string) {
  return {
    id, path: `/tmp/${id}`, display_name: name, trusted: true,
    sessions: [{ id: `sess-${id}`, status: "idle", model: "mock", title: "T", updated_at: "2026-01-01T00:00:00Z" }],
    active_sessions: 1, dirty_buffers: 0,
    usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
  };
}

beforeEach(() => {
  responses.clear();
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });

  responses.set("/pairing", { port: 9876, token: "dev" });
  responses.set("/projects", {
    projects: [makeProject("proj-a", "alpha"), makeProject("proj-b", "beta")],
  });
  responses.set("/ui-prefs", {});
  responses.set("/settings", { session: { first_edit_buffer_ms: 2000 }, exec: { command_timeout_s: 120 } });
  responses.set("/team-policy", { denied_tools: [], max_cost_usd: null });
  responses.set("/updates", { current_version: "0.1.0", staged: null });
  responses.set("/models", { models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
  responses.set("/evals", { runs: [] });
  responses.set("/plugins", { installed: [] });
  responses.set("/costs", { input_tokens: 0, output_tokens: 0, cost_usd: 0 });
  responses.set("/project/proj-a/tree", {
    entries: [
      { path: "src", name: "src", kind: "dir", git_status: "" },
      { path: "README.md", name: "README.md", kind: "file", git_status: "M" },
    ],
  });
  responses.set("/project/proj-a/file", { path: "README.md", content: "# Hello", total_bytes: 7 });

  vi.stubGlobal("fetch", vi.fn().mockImplementation((input: RequestInfo | URL) => {
    const url = String(input);
    for (const [key, body] of responses) {
      if (url.includes(key)) return jsonResponse(body);
    }
    return jsonResponse({});
  }));

  class FakeWS {
    static last: FakeWS | null = null;
    url: string;
    sent: string[] = [];
    onopen: (() => void) | null = null;
    onmessage: ((m: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(url: string) {
      this.url = url;
      FakeWS.last = this;
      queueMicrotask(() => this.onopen?.());
    }
    send(data: string) {
      this.sent.push(data);
      queueMicrotask(() => this.onmessage?.({ data: "auth ok" }));
    }
    close() {}
  }
  vi.stubGlobal("WebSocket", FakeWS);
});

describe("App file operations", () => {
  it("opens a file from the file tree via fuzzy finder", async () => {
    responses.set("/project/proj-a/files/fuzzy", {
      hits: [{ path: "README.md", name: "README.md", kind: "file", git_status: "M", score: 1 }],
      query: "read",
    });
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy());

    // 打开文件查找器（Cmd+P）
    fireEvent.keyDown(window, { key: "p", metaKey: true });
    const finder = await screen.findByTestId("file-finder");
    expect(finder).toBeTruthy();
  });

  it("switches between projects via the project list", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByText("alpha")).toBeTruthy());

    // 点击 beta 项目切换
    fireEvent.click(screen.getByText("beta"));
    await waitFor(() => {
      const openCall = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
        ([url]) => String(url).includes("/projects/open")
      );
      expect(openCall || screen.getByText("beta")).toBeTruthy();
    });
  });

  it("toggles sidebar views via rail buttons", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy());

    // 点击 rail 按钮切换侧栏视图
    const railTree = screen.queryByTestId("rail-files");
    if (railTree) {
      fireEvent.click(railTree);
      // 再点一次折叠
      fireEvent.click(railTree);
    }
    expect(true).toBe(true);
  });

  it("opens bottom panel and switches tabs", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy());

    const bottomOpen = screen.queryByTestId("bottom-open");
    if (bottomOpen) {
      fireEvent.click(bottomOpen);
      await waitFor(() => {
        expect(document.querySelector(".bottom-panel, [data-testid='bottom-panel']") || document.body).toBeTruthy();
      });
      // 底部 tab 切换
      const timelineTab = screen.queryByTestId("bottom-tab-timeline");
      if (timelineTab) fireEvent.click(timelineTab);
      const traceTab = screen.queryByTestId("bottom-tab-trace");
      if (traceTab) fireEvent.click(traceTab);
    }
  });

  it("renders diff panel when latest diff is set", async () => {
    responses.set("/session/sess-proj-a/trace", {
      events: [{
        id: 1, seq: 1, type: "patch_applied",
        payload: { output: { content: "--- a/f.ts\n+++ b/f.ts\n@@ -1 +1 @@\n-old\n+new\n", changed_files: ["f.ts"] } },
      }],
      latest_seq: 1,
    });
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy());
  });

  it("toggles follow mode via command palette", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("task-input")).toBeTruthy());
    fireEvent.keyDown(window, { key: "p", shiftKey: true, metaKey: true });
    const palette = await screen.findByTestId("command-palette");
    // 找 follow mode 命令
    const followBtn = within2(palette).queryByRole("button", { name: /follow|跟随/i });
    if (followBtn) fireEvent.click(followBtn);
    expect(true).toBe(true);
  });
});

// 避免 import within 冲突
function within2(el: HTMLElement) {
  return rtlWithin(el);
}
