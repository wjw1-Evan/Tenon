// AgentPanel 补测：patch/dirty_conflict 回调 / injectedTask / 停止恢复。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(events: Array<Record<string, unknown>>, status = "idle") {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events, latest_seq: events.length }),
    getSession: vi.fn().mockResolvedValue({ session_id: "s1", status, latest_seq: events.length, outcome: null }),
    sendMessage: vi.fn().mockResolvedValue({ accepted: true }),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("AgentPanel callbacks", () => {
  it("patch_applied 事件触发 onLatestDiff 和 patchLines 回调", async () => {
    const onLatestDiff = vi.fn();
    const patchLines = vi.fn();
    const events = [{
      id: 1, seq: 1, type: "patch_applied",
      payload: {
        output: {
          content: "--- a/f.ts\n+++ b/f.ts\n@@ -1 +1 @@\n-old\n+new\n",
          changed_files: ["f.ts"],
        },
      },
    }];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" onLatestDiff={onLatestDiff} onPatchLines={patchLines} />);
    await waitFor(() => expect(onLatestDiff).toHaveBeenCalled());
    await waitFor(() => expect(patchLines).toHaveBeenCalledWith("f.ts", expect.anything()));
  });

  it("dirty_conflict 事件触发 onDirtyConflict 回调", async () => {
    const onDirtyConflict = vi.fn();
    const events = [{
      id: 1, seq: 1, type: "diagnostics",
      payload: { dirty_conflict: true, path: "a.ts", base: "b", ours: "o", theirs: "t" },
    }];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" onDirtyConflict={onDirtyConflict} />);
    await waitFor(() => expect(onDirtyConflict).toHaveBeenCalledWith(
      expect.objectContaining({ path: "a.ts", base: "b", ours: "o", theirs: "t" })
    ));
  });

  it("injectedTask 直接发送消息并清空输入", async () => {
    const api = mockApi([]);
    const injectedTask = { token: 1, text: "fix bug" };
    render(<AgentPanel api={api} t={t} sessionId="s1" injectedTask={injectedTask} />);
    await waitFor(() => expect(api.sendMessage).toHaveBeenCalledWith("s1", "fix bug"));
  });

  it("injectedTask 无 session 不发送", () => {
    const api = mockApi([]);
    render(<AgentPanel api={api} t={t} sessionId={null} injectedTask={{ token: 1, text: "x" }} />);
    expect(api.sendMessage).not.toHaveBeenCalled();
  });

  it("sessionId 为 null 时显示空态且不轮询", () => {
    const api = mockApi([]);
    render(<AgentPanel api={api} t={t} sessionId={null} />);
    expect(screen.getByTestId("agent-empty")).toBeTruthy();
    expect(api.trace).not.toHaveBeenCalled();
  });

  it("resume 控制命令在 paused 状态触发", async () => {
    const api = mockApi([], "paused");
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const resumeBtn = await screen.findByTestId("resume");
    fireEvent.click(resumeBtn);
    await waitFor(() => expect(api.control).toHaveBeenCalledWith("s1", "resume"));
  });

  it("decision 后流式文本清空显示空态", async () => {
    const events = [
      { id: 1, seq: 1, type: "model_delta", payload: { text: "streaming..." } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "done" } },
    ];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    // decision 清空 streamText → model-stream 不渲染
    await waitFor(() => {
      expect(screen.queryByTestId("model-stream")).toBeNull();
      expect(screen.getByTestId("agent-feed")).toBeTruthy();
    });
  });

  it("user_input 后流式文本清空显示空态", async () => {
    const events = [
      { id: 1, seq: 1, type: "model_delta", payload: { text: "old" } },
      { id: 2, seq: 2, type: "user_input", payload: { text: "new task" } },
    ];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await waitFor(() => {
      expect(screen.queryByTestId("model-stream")).toBeNull();
    });
  });
});
