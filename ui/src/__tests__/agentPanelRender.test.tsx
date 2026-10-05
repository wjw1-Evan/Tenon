// AgentPanel 事件渲染覆盖：各类事件卡片 / 错误 / direct_action。
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(events: Array<Record<string, unknown>>, status = "idle") {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events, latest_seq: events.length }),
    getSession: vi.fn().mockResolvedValue({ session_id: "s1", status, latest_seq: events.length, outcome: null }),
    sendMessage: vi.fn().mockResolvedValue({}),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("AgentPanel event rendering", () => {
  it("renders direct_action card", async () => {
    const events = [{ id: 1, seq: 1, type: "direct_action", payload: { tool: "bash", level: "B" } }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText(/bash/);
    expect(screen.getByText("B")).toBeTruthy();
  });

  it("renders command_run card", async () => {
    const events = [{ id: 1, seq: 1, type: "command_run", payload: { tool: "npm test" } }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText(/npm test/);
  });

  it("renders patch_applied card", async () => {
    const events = [{ id: 1, seq: 1, type: "patch_applied", payload: { tool: "apply_patch" } }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText(/apply_patch/);
  });

  it("renders error card", async () => {
    const events = [{ id: 1, seq: 1, type: "error", payload: { error: "something broke" } }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText(/something broke/);
  });

  it("renders diagnostics verification card", async () => {
    const events = [{ id: 1, seq: 1, type: "diagnostics", payload: { verification: "0 errors" } }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText("0 errors");
  });

  it("renders decision with first_edit flag", async () => {
    const events = [{ id: 1, seq: 1, type: "decision", payload: { intent: "writing", first_edit: true } }];
    render(<AgentPanel api={mockApi(events, "executing")} t={t} sessionId="s1" />);
    expect(await screen.findByTestId("turn-running")).toHaveTextContent("state.executing");
  });

  it("renders user_input card", async () => {
    const events = [{ id: 1, seq: 1, type: "user_input", payload: { text: "fix the bug" } }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText("fix the bug");
  });

  it("handles trace API errors gracefully", async () => {
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockRejectedValue(new Error("offline")),
      getSession: vi.fn().mockResolvedValue({ session_id: "s1", status: "idle", latest_seq: 0, outcome: null }),
    } as unknown as TenonApi;
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    // 不崩溃
    await waitFor(() => expect(api.trace).toHaveBeenCalled());
  });

  it("handles getSession error gracefully", async () => {
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockResolvedValue({ events: [], latest_seq: 0 }),
      getSession: vi.fn().mockRejectedValue(new Error("gone")),
    } as unknown as TenonApi;
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    // 不崩溃
    await waitFor(() => expect(api.getSession).toHaveBeenCalled());
  });

  it("onStateChange callback fires with status", async () => {
    const onStateChange = vi.fn();
    const api = mockApi([], "executing");
    render(<AgentPanel api={api} t={t} sessionId="s1" onStateChange={onStateChange} />);
    await waitFor(() => expect(onStateChange).toHaveBeenCalledWith("executing"));
  });

  it("unknown event types render nothing", () => {
    const events = [{ id: 1, seq: 1, type: "unknown_type", payload: {} }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    expect(screen.getByTestId("agent-feed")).toBeTruthy();
  });
});

describe("AgentPanel additional", () => {


  it("renders paused state with resume button", async () => {
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockResolvedValue({ events: [], latest_seq: 0 }),
      getSession: vi.fn().mockResolvedValue({ session_id: "s1", status: "paused", latest_seq: 0, outcome: null }),
      sendMessage: vi.fn(),
      control: vi.fn().mockResolvedValue({ ok: true }),
    } as unknown as TenonApi;
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const resumeBtn = await screen.findByTestId("resume");
    expect(resumeBtn).toBeTruthy();
  });
});
