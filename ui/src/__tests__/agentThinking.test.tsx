// 思考过程显示（§7.2 v1.210，Codex 形态）：reasoning_delta 聚合为折叠卡。
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

function mockApi(events: Array<Record<string, unknown>>, status = "done"): TenonApi {
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
      .mockResolvedValue({ session_id: "s1", status, latest_seq: events.length, outcome: null }),
    sendMessage: vi.fn().mockResolvedValue({}),
  } as unknown as TenonApi;
}

describe("思考过程显示（v1.210 §7.2）", () => {
  it("reasoning_delta 聚合为可折叠思考卡（默认收起，点击展开全文）", async () => {
    const events = [
      { id: 1, seq: 1, type: "reasoning_delta", payload: { text: "先分析依赖" } },
      { id: 2, seq: 2, type: "reasoning_delta", payload: { text: "，再定方案" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const card = await screen.findByTestId("turn-thinking");
    expect(card.textContent).toContain("thread.thinking");
    // 默认收起：全文不可见
    expect(card.textContent).not.toContain("先分析依赖");
    // 点击展开
    fireEvent.click(screen.getByTestId("thinking-toggle"));
    expect(card.textContent).toContain("先分析依赖，再定方案");
  });

  it("无思考事件不渲染卡片", async () => {
    render(<AgentPanel api={mockApi([{ id: 1, seq: 1, type: "error", payload: {} }])} t={t} sessionId="s1" />);
    await screen.findByTestId("agent-panel");
    expect(screen.queryByTestId("turn-thinking")).toBeNull();
  });
});
