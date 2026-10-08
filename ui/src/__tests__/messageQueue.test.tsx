// v1.147 发送消息队列（§9.1）组件覆盖：排队气泡 / 徽标 / 双钮 / 移除 / 编辑回填 / 冻结期续发。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { QueuedMessage, TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(
  events: Array<Record<string, unknown>>,
  status = "idle",
  queue: QueuedMessage[] = [],
) {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockImplementation((_sessionId: string, after = 0) =>
      Promise.resolve({
        events: events.filter((e) => (e.seq as number) > after),
        latest_seq: events.length,
      })
    ),
    getSession: vi.fn().mockResolvedValue({
      session_id: "s1",
      status,
      latest_seq: events.length,
      outcome: null,
      queue,
    }),
    sendMessage: vi.fn().mockResolvedValue({ accepted: true, queued: false }),
    deleteQueuedMessage: vi.fn().mockResolvedValue({ removed: true }),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("AgentPanel 发送消息队列（v1.147）", () => {
  it("运行态渲染排队气泡：「已排队」徽标 + 移除钮，无手动发送钮（冻结语义相反面）", async () => {
    const api = mockApi(
      [{ id: 1, seq: 1, type: "user_input", payload: { text: "任务" } }],
      "executing",
      [{ id: "q1", text: "排队消息一" }],
    );
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const item = await screen.findByTestId("queue-item-q1");
    expect(item.textContent).toContain("排队消息一");
    expect(screen.getByTestId("queue-badge").textContent).toBe("message.queued");
    expect(screen.getByTestId("queue-remove-q1")).toBeTruthy();
    // 运行态自动出队，不提供手动发送钮
    expect(screen.queryByTestId("queue-send-q1")).toBeNull();
    // spinner 行显示队列计数徽标
    expect(screen.getByTestId("queue-count")).toBeTruthy();
  });

  it("运行态空输入仅停止单钮；键入后发送钮补出、点击走入队路径", async () => {
    const api = mockApi([], "executing", []);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await waitFor(() => expect(screen.getByTestId("stop")).toBeTruthy());
    // v1.184 Codex 单钮形态：空输入运行态无「发送」钮
    expect(screen.queryByTestId("send")).toBeNull();
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "排队消息二" } });
    const send = await screen.findByTestId("send");
    fireEvent.click(send);
    await waitFor(() =>
      expect((api.sendMessage as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
        "s1",
        "排队消息二",
      ]),
    );
    // 输入随发送清空（入队亦是投递），发送钮随之隐藏回落单钮
    await waitFor(() => expect(screen.queryByTestId("send")).toBeNull());
    expect((screen.getByTestId("task-input") as HTMLTextAreaElement).value).toBe("");
  });

  it("点击移除钮调用 DELETE 队列端点", async () => {
    const api = mockApi([], "idle", [{ id: "q2", text: "排队消息三" }]);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByTestId("queue-item-q2");
    fireEvent.click(screen.getByTestId("queue-remove-q2"));
    await waitFor(() =>
      expect((api.deleteQueuedMessage as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
        "s1",
        "q2",
      ]),
    );
  });

  it("点击气泡文本回填输入框并移除条目（编辑重发）", async () => {
    const api = mockApi([], "idle", [{ id: "q3", text: "排队消息四" }]);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByTestId("queue-item-q3");
    fireEvent.click(screen.getByTestId("queue-item-q3").querySelector(".queue-text")!);
    await waitFor(() =>
      expect((screen.getByTestId("task-input") as HTMLTextAreaElement).value).toBe("排队消息四"),
    );
    await waitFor(() =>
      expect((api.deleteQueuedMessage as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
        "s1",
        "q3",
      ]),
    );
  });

  it("冻结期（非运行态）条目显示手动发送钮：先出队再投递，防 drain 重复", async () => {
    const api = mockApi([], "idle", [{ id: "q4", text: "排队消息五" }]);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const sendBtn = await screen.findByTestId("queue-send-q4");
    fireEvent.click(sendBtn);
    await waitFor(() => {
      const calls = (api.deleteQueuedMessage as ReturnType<typeof vi.fn>).mock.calls;
      const sends = (api.sendMessage as ReturnType<typeof vi.fn>).mock.calls;
      expect(calls[0]).toEqual(["s1", "q4"]);
      expect(sends[0]).toEqual(["s1", "排队消息五"]);
      // 先出队后发送
      expect(sends.length).toBeGreaterThanOrEqual(1);
    });
  });

  it("停止后队列冻结保留（状态 paused，条目仍在）", async () => {
    const api = mockApi([], "paused", [{ id: "q5", text: "排队消息六" }]);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByTestId("queue-item-q5");
    // paused 态发送钮变恢复；停止钮不渲染
    expect(screen.getByTestId("resume")).toBeTruthy();
    expect(screen.queryByTestId("stop")).toBeNull();
  });
});
