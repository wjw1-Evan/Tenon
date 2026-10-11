// v2.7 diff 级反馈回路 App 级接线（design-v2.md §6 / design.md §7.2）：
// 运行中会话（审查面自动停靠）→ trace patch_applied 落 latestDiff →
// 审查面选中 diff 行 → 反馈卡输入发送 → 经行内指令同款链路 POST /session/:id/message
// （运行态自动入队，回合完成续跑——「边跑边审、审完即改」闭环的 UI 半边）。
// v2.8 审查摘要卡：trace 事件流（subtasks / patch_applied / diagnostics.verification）
// 经 AgentPanel digest 上提 → 审查面摘要区渲染。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import type { ProjectSummary } from "../lib/api";

function usage() {
  return { input_tokens: 0, output_tokens: 0, cost_usd: 0 };
}
function sessionSummary(status: string) {
  return {
    id: "s-a",
    status,
    model: "mock",
    updated_at: "2026-01-01T00:00:00Z",
  };
}
function project(status: string): ProjectSummary {
  return {
    id: "proj-a",
    path: "/tmp/proj-a",
    display_name: "project-a",
    trusted: true,
    sessions: [sessionSummary(status)],
    session_runtimes: ["s-a"],
    active_sessions: 1,
    dirty_buffers: 0,
    usage: usage(),
  };
}

const PATCH_DIFF = "--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1,3 +1,3 @@\n ctx\n-old\n+new\n ctx2";

let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (k in Object.fromEntries(backing) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
  fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    if (url.endsWith("/projects/open")) return jsonResponse(project("executing"));
    if (url.endsWith("/projects")) return jsonResponse({ projects: [project("executing")] });
    if (url.endsWith("/session") && method === "POST")
      return jsonResponse({ session_id: "new-sess", project_id: "?" });
    if (url.includes("/session/s-a/trace"))
      return jsonResponse({
        events: [
          {
            seq: 1,
            type: "subtasks",
            payload: {
              items: [
                { title: "write the file", status: "done" },
                { title: "verify the file", status: "in_progress" },
              ],
            },
          },
          {
            seq: 2,
            type: "patch_applied",
            payload: { output: { content: `已写入\n${PATCH_DIFF}`, changed_files: ["src/app.ts"] } },
          },
          {
            seq: 3,
            type: "diagnostics",
            payload: { verification: "no new diagnostics" },
          },
        ],
      });
    if (url.endsWith("/session/s-a"))
      return jsonResponse({ session_id: "s-a", status: "executing", latest_seq: 1, outcome: null });
    if (url.endsWith("/session/new-sess"))
      return jsonResponse({ session_id: "new-sess", status: "idle", latest_seq: 0, outcome: null });
    if (url.includes("/ui-state")) return jsonResponse({});
    if (url.includes("/pairing")) return jsonResponse({ port: 9876, token: "dev" });
    if (url.includes("/ui-prefs")) return jsonResponse({});
    if (url.includes("/settings")) return jsonResponse({});
    if (url.includes("/team-policy")) return jsonResponse({ denied_tools: [], max_cost_usd: null });
    if (url.includes("/updates")) return jsonResponse({ current_version: "0.1.0", staged: null });
    if (url.includes("/models"))
      return jsonResponse({ models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
    if (url.includes("/evals")) return jsonResponse({ runs: [] });
    if (url.includes("/costs")) return jsonResponse(usage());
    return jsonResponse({});
  });
  vi.stubGlobal("fetch", fetchMock);
  class FakeWS {
    onopen: (() => void) | null = null;
    onmessage: ((m: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(_url: string) {
      queueMicrotask(() => this.onopen?.());
    }
    send(_data: string) {
      queueMicrotask(() => this.onmessage?.({ data: "auth ok" }));
    }
    close() {}
  }
  vi.stubGlobal("WebSocket", FakeWS);
});

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

describe("审查面 diff 级反馈 App 接线（v2.7）", () => {
  it("运行中审查面选行反馈 → POST /session/:id/message 携带任务组装", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    // 审查面停靠且 diff 到流（AgentPanel 500ms trace 轮询消费 patch_applied；
    // diffFromPatchEvent 保留输出前缀行——行序：0 已写入 1 --- 2 +++ 3 @@ 4 ctx 5 -old 6 +new）
    await waitFor(
      () => expect(screen.getByText("+new")).toBeTruthy(),
      { timeout: 5000 }
    );
    fireEvent.click(screen.getAllByTestId("diff-line")[6]);
    await waitFor(() => expect(screen.getByTestId("review-feedback")).toBeTruthy());
    fireEvent.change(screen.getByTestId("review-fb-input"), {
      target: { value: "变量命名改成更明确的形式" },
    });
    fireEvent.click(screen.getByTestId("review-fb-send"));
    await waitFor(() => {
      const call = fetchMock.mock.calls.find(([u, i]) => {
        const init = i as RequestInit | undefined;
        return String(u).endsWith("/session/s-a/message") && (init?.method ?? "").toUpperCase() === "POST";
      });
      expect(call).toBeTruthy();
      const body = JSON.parse(String((call![1] as RequestInit).body));
      // 任务组装：反馈标记 + 摘录（默认英文源语言文案）
      expect(body.text).toContain("Review feedback");
      expect(body.text).toContain("src/app.ts");
      expect(body.text).toContain("+new");
    });
  });

  it("v2.8 审查摘要卡：事件流上提渲染子任务 / 产物 / 证据", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("review-pane")).toBeTruthy(), { timeout: 5000 });
    const digest = await waitFor(() => {
      const node = screen.getByTestId("review-digest");
      expect(node).toBeTruthy();
      return node;
    });
    expect(screen.getByTestId("review-subtasks")).toHaveTextContent("1/2");
    expect(screen.getByTestId("review-subtasks")).toHaveTextContent("verify the file");
    expect(screen.getByTestId("review-artifacts")).toHaveTextContent("src/app.ts");
    expect(screen.getByTestId("review-evidence")).toHaveTextContent("no new diagnostics");
    expect(digest).toBeTruthy();
  });
});
