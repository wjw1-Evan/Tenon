import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };

describe("AgentPanel streaming", () => {
  it("joins model deltas into one live card and hides raw delta cards", async () => {
    const deltaEvents = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "model_delta", payload: { text: "正在" } },
      { id: 3, seq: 3, type: "model_delta", payload: { text: "流式回答" } },
    ];
    let call = 0;
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockImplementation(() => {
        call += 1;
        return Promise.resolve(
          call === 1 ? { events: deltaEvents, latest_seq: 3 } : { events: [], latest_seq: 3 }
        );
      }),
      getSession: vi.fn().mockResolvedValue({
        session_id: "s1",
        status: "deciding",
        latest_seq: 3,
        outcome: null,
      }),
    } as unknown as TenonApi;
    const t = (key: string) => key;
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await waitFor(() => expect(screen.getByTestId("model-stream")).toHaveTextContent("正在流式回答"));
    expect(screen.queryAllByText("正在流式回答")).toHaveLength(1);
  });
});

describe("发送按钮状态化（v1.59）", () => {
  function mockApi(status: string) {
    return {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockResolvedValue({ events: [], latest_seq: 0 }),
      getSession: vi.fn().mockResolvedValue({
        session_id: "s1",
        status,
        latest_seq: 0,
        outcome: null,
      }),
      sendMessage: vi.fn().mockResolvedValue({}),
      control: vi.fn().mockResolvedValue({ ok: true }),
    } as unknown as TenonApi;
  }
  const t = (key: string) => key;

  it("推进态显示「停止」并派发 stop 控制命令（受理后防连点）", async () => {
    const api = mockApi("executing");
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const stopBtn = await screen.findByTestId("stop");
    expect(stopBtn).toHaveTextContent("message.stop_short");
    fireEvent.click(stopBtn);
    await waitFor(() => expect(api.control).toHaveBeenCalledWith("s1", "stop"));
    expect(stopBtn).toBeDisabled();
  });

  it("空闲态显示「发送」并照常派发消息", async () => {
    const api = mockApi("idle");
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const sendBtn = await screen.findByTestId("send");
    expect(sendBtn).toHaveTextContent("message.send");
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "hi" } });
    fireEvent.click(sendBtn);
    await waitFor(() => expect(api.sendMessage).toHaveBeenCalledWith("s1", "hi"));
  });
});

describe("会话切换事件流重置", () => {
  const t = (key: string) => key;

  it("sessionId 变化后清空旧会话事件并显示空态，不串会话", async () => {
    const oldEvents = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "旧会话消息" } },
    ];
    let currentSession = "s1";
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockImplementation((_id: string) =>
        Promise.resolve(
          currentSession === "s1"
            ? { events: oldEvents, latest_seq: 1 }
            : { events: [], latest_seq: 0 }
        )
      ),
      getSession: vi.fn().mockResolvedValue({
        session_id: currentSession,
        status: "done",
        latest_seq: 0,
        outcome: null,
      }),
    } as unknown as TenonApi;
    const { rerender } = render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByText("旧会话消息");

    currentSession = "s2";
    rerender(<AgentPanel api={api} t={t} sessionId="s2" />);
    // 旧会话内容不得残留；空会话显示空态引导。
    expect(screen.queryByText("旧会话消息")).toBeNull();
    expect(await screen.findByTestId("agent-empty")).toHaveTextContent("thread.empty");
  });

  it("决策卡为 JSON 时解出 answer 字段呈现，非 JSON 原样", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "decision",
        payload: {
          intent: '{"intent":"创建文件","answer":"文件已创建，内容正确。","needs_change":false}',
        },
      },
    ];
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockResolvedValue({ events, latest_seq: 1 }),
      getSession: vi.fn().mockResolvedValue({
        session_id: "s1",
        status: "done",
        latest_seq: 1,
        outcome: null,
      }),
    } as unknown as TenonApi;
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByText("文件已创建，内容正确。");
    expect(screen.queryByText(/needs_change/)).toBeNull();
  });
});
