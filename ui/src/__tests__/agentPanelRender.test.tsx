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
    // 尊重 after 游标：固定返回全量会让 500ms 轮询重复追加同一事件，
    // 高负载下第二个轮询先于断言到达即出现重复卡片（实测抖动）。
    trace: vi.fn().mockImplementation((_sessionId: string, after = 0) =>
      Promise.resolve({
        events: events.filter((e) => (e.seq as number) > after),
        latest_seq: events.length,
      })
    ),
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

  it("renders laya_decide badge for agent_tool decider_call (v1.124)", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "decider_call",
        payload: {
          feature: "agent_tool",
          kind: "choice",
          result: { label: "needs_change", confidence: 0.87 },
          duration_ms: 3,
          origin: "agent_tool",
        },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const badge = await screen.findByTestId("turn-laya");
    expect(badge.textContent).toContain("needs_change");
    expect(badge.textContent).toContain("3ms");
  });

  it("does not render daemon-auto decider_call (v1.124 仅轨迹可见)", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "decider_call",
        payload: { feature: "risk", kind: "score", result: 0.83, rule: true },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await waitFor(() => expect(screen.queryByTestId("turn-laya")).toBeNull());
  });

  it("collapses laya_decide step card into read-only aggregate (v1.124)", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "command_run",
        payload: { tool: "laya_decide", output: { ok: true, content: "{}" } },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    // 步骤卡被折叠；计数也由徽标承载（聚合行不出现）
    await waitFor(() => {
      expect(screen.queryByTestId("turn-step")).toBeNull();
      expect(screen.queryByTestId("turn-readonly")).toBeNull();
    });
  });

  it("renders subtasks card with only the latest snapshot per turn (v1.146)", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "重构导出功能" } },
      {
        id: 2,
        seq: 2,
        type: "subtasks",
        payload: {
          items: [
            { title: "改造导出管道", status: "done" },
            { title: "补充单测", status: "in_progress" },
          ],
        },
      },
      {
        id: 3,
        seq: 3,
        type: "subtasks",
        payload: {
          items: [
            { title: "改造导出管道", status: "done" },
            { title: "补充单测", status: "done" },
          ],
        },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    // 同回合更早的状态演进不渲染，只出最新一次快照
    const cards = await screen.findAllByTestId("turn-subtasks");
    expect(cards.length).toBe(1);
    expect(cards[0].textContent).toContain("2/2");
    expect(cards[0].textContent).toContain("补充单测");
    expect(cards[0].getAttribute("data-done")).toBe("true");
  });

  it("renders in-progress subtasks card and suppresses its step card (v1.146)", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "多步任务" } },
      {
        id: 2,
        seq: 2,
        type: "subtasks",
        payload: {
          items: [
            { title: "第一步", status: "done" },
            { title: "第二步", status: "in_progress" },
            { title: "第三步", status: "pending" },
          ],
        },
      },
      // command_run 审计记录照只读聚合排除，不落步骤卡
      {
        id: 3,
        seq: 3,
        type: "command_run",
        payload: { tool: "subtasks", output: { ok: true, content: "ok" } },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const card = await screen.findByTestId("turn-subtasks");
    expect(card.getAttribute("data-done")).toBe("false");
    expect(card.textContent).toContain("1/3");
    expect(card.textContent).toContain("第三步");
    await waitFor(() => {
      expect(screen.queryByTestId("turn-step")).toBeNull();
      expect(screen.queryByTestId("turn-readonly")).toBeNull();
    });
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
