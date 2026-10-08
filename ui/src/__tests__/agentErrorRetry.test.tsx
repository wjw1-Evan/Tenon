// 错误回合一键重试（§7.5 v1.204）：错误行渲染重试钮 + 点击重发该回合原始任务文本。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const t = (key: string) => key;

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
});
afterEach(() => vi.unstubAllGlobals());

function mockApi(events: Array<Record<string, unknown>>, sendMessage = vi.fn()): TenonApi {
  return {
    models: vi.fn().mockResolvedValue({ models: [], default: "", laya: null }),
    trace: vi.fn().mockImplementation((_sessionId: string, after = 0) =>
      Promise.resolve({
        events: events.filter((e) => (e.seq as number) > after),
        latest_seq: events.length,
      }),
    ),
    getSession: vi
      .fn()
      .mockResolvedValue({ session_id: "s1", status: "error", latest_seq: events.length, outcome: null }),
    sendMessage,
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("错误回合一键重试（v1.204 §7.5）", () => {
  it("error 事件行渲染重试钮", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "修复登录 bug" } },
      { id: 2, seq: 2, type: "error", payload: { error: "HTTP 500: 上游故障" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const btn = await screen.findByTestId("turn-error-retry");
    expect(btn.textContent).toContain("thread.retry");
  });

  it("点击重试重发该回合原始任务文本（不经草稿框）", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "修复登录 bug" } },
      { id: 2, seq: 2, type: "error", payload: { error: "HTTP 500: 上游故障" } },
    ];
    const sendMessage = vi.fn().mockResolvedValue({});
    render(<AgentPanel api={mockApi(events, sendMessage)} t={t} sessionId="s1" />);
    await screen.findByTestId("turn-error-retry");
    fireEvent.click(screen.getByTestId("turn-error-retry"));
    await waitFor(() => expect(sendMessage).toHaveBeenCalled());
    expect(sendMessage.mock.calls[0][1]).toBe("修复登录 bug");
  });

  it("拒绝类错误行不渲染重试钮（denied 非模型失败）", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "越权写入" } },
      { id: 2, seq: 2, type: "error", payload: { denied: "write_file", reason: "只读会话" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText((_, el) => el?.className === "turn-error");
    expect(screen.queryByTestId("turn-error-retry")).toBeNull();
  });
});
