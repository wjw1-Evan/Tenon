// v2.33 Enter 直发任务（§7.4）：任务输入框 Enter 直发 / Shift+Enter 换行 /
// IME 组合态不误发 / Cmd+Enter 兼容保留；@ 与 / 浮层开启时 Enter 优先选补全
// 的优先语义由 mention.test.tsx 覆盖。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi() {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events: [], latest_seq: 0 }),
    getSession: vi.fn().mockResolvedValue({
      session_id: "s1",
      status: "idle",
      latest_seq: 0,
      outcome: null,
      queue: [],
    }),
    sendMessage: vi.fn().mockResolvedValue({ accepted: true, queued: false }),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

function renderPanel() {
  const api = mockApi();
  render(<AgentPanel api={api} t={t} sessionId="s1" projectId="p1" />);
  const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
  return { api, input };
}

describe("任务输入框 Enter 直发（v2.33）", () => {
  it("Enter 直发并清空输入", async () => {
    const { api, input } = renderPanel();
    fireEvent.change(input, { target: { value: "hello task" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await vi.waitFor(() =>
      expect(api.sendMessage).toHaveBeenCalledWith("s1", "hello task"),
    );
    await vi.waitFor(() => expect(input.value).toBe(""));
  });

  it("Shift+Enter 换行不发送", () => {
    const { api, input } = renderPanel();
    fireEvent.change(input, { target: { value: "line" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(api.sendMessage).not.toHaveBeenCalled();
    expect(input.value).toBe("line");
  });

  it("IME 组合态（keyCode 229）Enter 仅确认候选不发送", () => {
    const { api, input } = renderPanel();
    fireEvent.change(input, { target: { value: "nihongo" } });
    fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });
    expect(api.sendMessage).not.toHaveBeenCalled();
  });

  it("Cmd+Enter 兼容保留同语义发送", async () => {
    const { api, input } = renderPanel();
    fireEvent.change(input, { target: { value: "compat" } });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    await vi.waitFor(() =>
      expect(api.sendMessage).toHaveBeenCalledWith("s1", "compat"),
    );
  });
});
