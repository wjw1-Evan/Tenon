// AgentTrace 面板测试：token 度量 / 工具调用表 / 风险动作计数 / token 速度采样。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentTracePanel } from "../components/AgentTracePanel";
import type { TenonApi } from "../lib/api";

const traceMock = vi.fn();
const costsMock = vi.fn();

function api() {
  return { trace: traceMock, costs: costsMock } as unknown as TenonApi;
}

beforeEach(() => {
  traceMock.mockReset();
  costsMock.mockReset();
  vi.useFakeTimers({ shouldAdvanceTime: true });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("AgentTracePanel", () => {
  it("renders empty state when sessionId is null", () => {
    render(<AgentTracePanel api={api()} sessionId={null} />);
    expect(screen.getByTestId("agent-trace")).toBeTruthy();
    expect(screen.getByText("AgentTrace（本地审计）")).toBeTruthy();
    expect(screen.getByText("输入 0 tok")).toBeTruthy();
    expect(screen.getByText("成本 $0.0000")).toBeTruthy();
    expect(traceMock).not.toHaveBeenCalled();
  });

  it("loads trace events and cost metrics for an active session", async () => {
    traceMock.mockResolvedValue({
      events: [
        { id: 1, seq: 1, type: "patch_applied", payload: { tool: "apply_patch", output: { ok: true } } },
        { id: 2, seq: 2, type: "command_run", payload: { tool: "bash", output: { ok: false } } },
        { id: 3, seq: 3, type: "direct_action", payload: { risk: "write" } },
        { id: 4, seq: 4, type: "model_response", payload: { text: "hello world" } },
      ],
    });
    costsMock.mockResolvedValue({ input_tokens: 1200, output_tokens: 340, cost_usd: 0.0042 });

    render(<AgentTracePanel api={api()} sessionId="s1" />);

    await waitFor(() => {
      expect(screen.getByText("输入 1200 tok")).toBeTruthy();
    });
    expect(screen.getByText("输出 340 tok")).toBeTruthy();
    expect(screen.getByText("成本 $0.0042")).toBeTruthy();
    expect(screen.getByText("风险动作 1 次")).toBeTruthy();
    // 工具调用明细：patch + command 均出现
    expect(screen.getByText("apply_patch")).toBeTruthy();
    expect(screen.getByText("bash")).toBeTruthy();
    // 非 tool 事件显示 JSON 截断
    expect(screen.getByText(/risk/)).toBeTruthy();
  });

  it("computes token speed from consecutive polling windows", async () => {
    traceMock.mockResolvedValue({ events: [] });
    costsMock
      .mockResolvedValueOnce({ input_tokens: 100, output_tokens: 50, cost_usd: 0 })
      .mockResolvedValueOnce({ input_tokens: 300, output_tokens: 150, cost_usd: 0 });

    render(<AgentTracePanel api={api()} sessionId="s2" />);
    await waitFor(() => expect(screen.getByText("输入 100 tok")).toBeTruthy());

    // 推进 1.5s 触发下一次轮询
    vi.advanceTimersByTime(1600);
    await waitFor(() => expect(screen.getByText("输入 300 tok")).toBeTruthy());
    // 速度指示应出现（增量 > 0）
    await waitFor(() => {
      expect(screen.getByTestId("inp-speed")).toBeTruthy();
    });
    expect(screen.getByTestId("out-speed")).toBeTruthy();
  });

  it("handles API errors gracefully", async () => {
    traceMock.mockRejectedValue(new Error("offline"));
    render(<AgentTracePanel api={api()} sessionId="s3" />);
    // 不崩溃、保持空指标
    expect(screen.getByText("输入 0 tok")).toBeTruthy();
    vi.advanceTimersByTime(1600);
    expect(screen.getByText("输入 0 tok")).toBeTruthy();
  });
});
