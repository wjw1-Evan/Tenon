// v1.129 模型用量观测：回合页脚缓存命中率 / 输出速度徽标 + 轨迹汇总行。
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import { AgentTracePanel } from "../components/AgentTracePanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(events: Array<Record<string, unknown>>, costs: Record<string, unknown> = {}) {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockImplementation((_sessionId: string, after = 0) =>
      Promise.resolve({
        events: events.filter((e) => (e.seq as number) > after),
        latest_seq: events.length,
      })
    ),
    getSession: vi.fn().mockResolvedValue({ session_id: "s1", status: "done", latest_seq: events.length, outcome: null }),
    costs: vi.fn().mockResolvedValue(costs),
    sendMessage: vi.fn().mockResolvedValue({}),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("turn usage badge (v1.129)", () => {
  it("renders aggregated cache hit rate and output speed at turn footer", async () => {
    // 两个模型回合：mock cached = input/2；聚合命中率 50%，速度按总耗时折算
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      {
        id: 2, seq: 2, type: "decision",
        payload: { intent: "第一回合", usage: { input_tokens: 100, output_tokens: 20, cached_input_tokens: 50, duration_ms: 1000 } },
      },
      {
        id: 3, seq: 3, type: "decision",
        payload: { intent: "最终回答", usage: { input_tokens: 60, output_tokens: 40, cached_input_tokens: 30, duration_ms: 1000 } },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const badge = await screen.findByTestId("turn-usage");
    expect(badge.textContent).toContain("↑ 160 tok");
    expect(badge.textContent).toContain("cached 50%");
    // 60 output / 2s = 30 tok/s
    expect(badge.textContent).toContain("30.0 tok/s");
  });

  it("omits cache segment when nothing cached (Anthropic without cache_control)", async () => {
    const events = [
      {
        id: 1, seq: 1, type: "decision",
        payload: { intent: "回答", usage: { input_tokens: 100, output_tokens: 20, cached_input_tokens: 0, duration_ms: 500 } },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const badge = await screen.findByTestId("turn-usage");
    expect(badge.textContent).not.toContain("cached");
    expect(badge.textContent).toContain("40.0 tok/s");
  });

  it("renders nothing without usage payload (legacy daemon events)", async () => {
    const events = [{ id: 1, seq: 1, type: "decision", payload: { intent: "旧事件无 usage" } }];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText("旧事件无 usage");
    expect(screen.queryByTestId("turn-usage")).toBeNull();
  });
});

describe("trace metrics cache row (v1.129)", () => {
  it("shows session cache hit rate and average speed from /costs", async () => {
    render(
      <AgentTracePanel
        api={mockApi([], { input_tokens: 200, output_tokens: 100, cached_tokens: 150, duration_ms: 2000, cost_usd: 0.01 })}
        t={t}
        sessionId="s1"
      />
    );
    const cache = await screen.findByTestId("trace-cache");
    expect(cache.textContent).toContain("75%");
    const avg = await screen.findByTestId("trace-avg-speed");
    expect(avg.textContent).toContain("50.0 tok/s");
  });

  it("hides cache row when nothing cached", async () => {
    render(
      <AgentTracePanel
        api={mockApi([], { input_tokens: 200, output_tokens: 100, cached_tokens: 0, duration_ms: 0, cost_usd: 0.01 })}
        t={t}
        sessionId="s1"
      />
    );
    await screen.findByTestId("trace-metrics");
    expect(screen.queryByTestId("trace-cache")).toBeNull();
    expect(screen.queryByTestId("trace-avg-speed")).toBeNull();
  });
});
