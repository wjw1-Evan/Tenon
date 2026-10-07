// v1.161 输入内 / 斜杠命令（§7.5）组件覆盖：触发词提取纯函数 / 浮层开合与过滤 /
// /compact 发 control / /review 注入审查任务走发送链 / Esc 收层 / 会话态命令可用性。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel, extractSlashCommand } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(status = "idle") {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events: [], latest_seq: 0 }),
    getSession: vi.fn().mockResolvedValue({
      session_id: "s1",
      status,
      latest_seq: 0,
      outcome: null,
      queue: [],
    }),
    sendMessage: vi.fn().mockResolvedValue({ accepted: true, queued: false }),
    control: vi.fn().mockResolvedValue({ ok: true }),
    fuzzyFiles: vi.fn().mockResolvedValue({ hits: [], query: "" }),
  } as unknown as TenonApi;
}

describe("extractSlashCommand 触发词提取（纯函数）", () => {
  it("首字符 / 触发，返回小写过滤词；空白即闭合", () => {
    expect(extractSlashCommand("/", 1)).toBe("");
    expect(extractSlashCommand("/Co", 3)).toBe("co");
    expect(extractSlashCommand("/compact tail", 13)).toBeNull();
  });

  it("非开头 / 与无 / 不触发", () => {
    expect(extractSlashCommand("a/b", 3)).toBeNull();
    expect(extractSlashCommand("compact", 7)).toBeNull();
  });
});

describe("AgentPanel 斜杠命令菜单（v1.161）", () => {
  it("键入 / 弹命令浮层（会话态双命令）；过滤 /re 只剩 review", async () => {
    const api = mockApi();
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "/" } });
    await screen.findByTestId("slash-menu");
    expect(screen.getByTestId("slash-item-compact")).toBeTruthy();
    expect(screen.getByTestId("slash-item-review")).toBeTruthy();
    fireEvent.change(input, { target: { value: "/re" } });
    await waitFor(() => expect(screen.queryByTestId("slash-item-compact")).toBeNull());
    expect(screen.getByTestId("slash-item-review")).toBeTruthy();
  });

  it("Enter 执行 /compact：清空输入并发 compact 控制命令", async () => {
    const api = mockApi();
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "/compact" } });
    await screen.findByTestId("slash-menu");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.value).toBe("");
    await waitFor(() =>
      expect(api.control).toHaveBeenCalledWith("s1", "compact"),
    );
  });

  it("Enter 执行 /review：清空输入并注入审查任务文本走 sendMessage", async () => {
    const api = mockApi();
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "/review" } });
    await screen.findByTestId("slash-menu");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.value).toBe("");
    await waitFor(() =>
      expect((api.sendMessage as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
        "s1",
        "slash.review_task",
      ]),
    );
  });

  it("Esc 仅收浮层不 blur；草稿无会话不提供 /compact", async () => {
    const api = mockApi();
    const { unmount } = render(
      <AgentPanel api={api} t={t} sessionId="s1" />,
    );
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    input.focus();
    fireEvent.change(input, { target: { value: "/" } });
    await screen.findByTestId("slash-menu");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByTestId("slash-menu")).toBeNull();
    expect(document.activeElement).toBe(input);
    unmount();

    // 草稿态：无会话只有 review 可用
    const api2 = mockApi();
    render(
      <AgentPanel
        api={api2}
        t={t}
        sessionId={null}
        draft
        onDraftSend={vi.fn().mockResolvedValue(undefined)}
      />,
    );
    const input2 = screen.getByTestId("task-input") as HTMLTextAreaElement;
    fireEvent.change(input2, { target: { value: "/" } });
    await screen.findByTestId("slash-menu");
    expect(screen.queryByTestId("slash-item-compact")).toBeNull();
    expect(screen.getByTestId("slash-item-review")).toBeTruthy();
  });
});
