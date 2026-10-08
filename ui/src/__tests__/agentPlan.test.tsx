// 计划模式 UI（§9.2 v1.179）：计划卡渲染 + 批准按钮发送批准文本。
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
      .mockResolvedValue({ session_id: "s1", status: "paused", latest_seq: events.length, outcome: null }),
    sendMessage,
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("计划模式线程卡（v1.179 §9.2）", () => {
  it("渲染计划 items 清单与批准按钮", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "plan_submitted",
        payload: { items: ["改 A 文件支持 X", "补测试并跑通"] },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const card = await screen.findByTestId("turn-plan");
    expect(card.textContent).toContain("thread.plan_title");
    expect(card.textContent).toContain("改 A 文件支持 X");
    expect(card.textContent).toContain("补测试并跑通");
    expect(screen.getByTestId("plan-approve")).toBeTruthy();
  });

  it("批准按钮发送批准文本（新回合按计划执行）", async () => {
    const events = [
      { id: 1, seq: 1, type: "plan_submitted", payload: { items: ["步骤一"] } },
    ];
    const sendMessage = vi.fn().mockResolvedValue({});
    render(<AgentPanel api={mockApi(events, sendMessage)} t={t} sessionId="s1" />);
    await screen.findByTestId("turn-plan");
    fireEvent.click(screen.getByTestId("plan-approve"));
    await waitFor(() => expect(sendMessage).toHaveBeenCalled());
    expect(sendMessage.mock.calls[0][1]).toBe("thread.plan_approve_text");
  });
});
