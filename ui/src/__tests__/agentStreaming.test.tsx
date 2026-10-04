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
