// v2.60 用户消息图片回显：气泡缩略图行（blob URL 拉取/回收）+ 全屏灯箱开合。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

// jsdom 无 blob URL API（同 localStorage 缺失一类），补桩供组件回收逻辑调用
URL.createObjectURL ??= () => "blob:stub";
URL.revokeObjectURL ??= () => {};

function mockApi(events: Array<Record<string, unknown>>) {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events, latest_seq: events.length }),
    getSession: vi.fn().mockResolvedValue({
      session_id: "s1",
      status: "idle",
      latest_seq: events.length,
      outcome: null,
    }),
    sendMessage: vi.fn().mockResolvedValue({}),
    control: vi.fn().mockResolvedValue({ ok: true }),
    attachmentObjectUrl: vi
      .fn()
      .mockImplementation((_sid: string, name: string) => Promise.resolve(`blob:mock/${name}`)),
  } as unknown as TenonApi;
}

describe("用户消息图片回显（v2.60）", () => {
  it("气泡下渲染附件缩略图（逐个拉取对象 URL），点击开灯箱，✕ 关闭", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "user_input",
        payload: { text: "看图", attachments: ["0.png", "1.jpeg"] },
      },
      { id: 2, seq: 2, type: "decision", payload: { intent: "收到" } },
    ];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByText("看图");
    await waitFor(() => expect(screen.getByTestId("turn-image-0")).toBeTruthy());
    await waitFor(() => expect(screen.getByTestId("turn-image-1")).toBeTruthy());
    const calls = (api.attachmentObjectUrl as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.map((c) => c[1])).toEqual(["0.png", "1.jpeg"]);
    // 点击缩略图 → 灯箱；✕ 关闭
    fireEvent.click(screen.getByTestId("turn-image-0"));
    expect(screen.getByTestId("image-lightbox")).toBeTruthy();
    fireEvent.click(screen.getByTestId("image-lightbox-close"));
    expect(screen.queryByTestId("image-lightbox")).toBeNull();
  });

  it("Esc 关闭灯箱", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "user_input",
        payload: { text: "看图", attachments: ["0.png"] },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const thumb = await screen.findByTestId("turn-image-0");
    fireEvent.click(thumb);
    expect(screen.getByTestId("image-lightbox")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("image-lightbox")).toBeNull();
  });

  it("旧会话事件无 attachments 字段：不渲染缩略图行也不调拉取", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "纯文本" } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "好" } },
    ];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByText("纯文本");
    expect(screen.queryByTestId("turn-images")).toBeNull();
    expect(api.attachmentObjectUrl).not.toHaveBeenCalled();
  });
});
