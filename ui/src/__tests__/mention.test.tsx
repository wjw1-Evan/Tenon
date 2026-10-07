// v1.159 输入内 @ 文件引用补全（§7.5）组件覆盖：触发词提取 / 浮层开合 /
// 键盘选择（↑↓ Enter Tab Esc）/ 鼠标选中插入 / 无项目不触发。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { AgentPanel, extractMentionQuery } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

const fuzzyHits = [
  { path: "src/app.tsx", name: "app.tsx", kind: "file", git_status: "", score: 90 },
  { path: "src/agent", name: "agent", kind: "dir", git_status: "", score: 60 },
];

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
    fuzzyFiles: vi.fn().mockResolvedValue({ hits: fuzzyHits, query: "" }),
  } as unknown as TenonApi;
}

describe("extractMentionQuery 触发词提取（纯函数）", () => {
  it("文本首 @ 触发，query 为 @ 后至光标片段", () => {
    expect(extractMentionQuery("@sr", 3)).toEqual({ start: 0, query: "sr" });
  });

  it("空白后 @ 触发；@ 与光标间遇空白即闭合", () => {
    expect(extractMentionQuery("see @src ", 9)).toBeNull();
    expect(extractMentionQuery("see @src", 8)).toEqual({ start: 4, query: "src" });
  });

  it("a@b 邮箱形态（@ 前非空白）不触发", () => {
    expect(extractMentionQuery("a@b", 3)).toBeNull();
  });
});

describe("AgentPanel @ 文件引用补全（v1.159）", () => {
  it("键入 @ 弹出补全浮层并按过滤词请求 fuzzy", async () => {
    const api = mockApi();
    render(<AgentPanel api={api} t={t} sessionId="s1" projectId="p1" />);
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "@sr" } });
    const menu = await screen.findByTestId("mention-menu");
    expect(menu.getAttribute("aria-label")).toBe("mention.title");
    await vi.waitFor(() =>
      expect((api.fuzzyFiles as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual(["p1", "sr", 8]),
    );
    expect(screen.getByTestId("mention-item-0").textContent).toContain("src/app.tsx");
  });

  it("↑↓ 移动高亮，Enter 选中插入 @路径（trailing 空格）", async () => {
    const api = mockApi();
    render(<AgentPanel api={api} t={t} sessionId="s1" projectId="p1" />);
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "@" } });
    await screen.findByTestId("mention-menu");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByTestId("mention-item-1").getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.value).toBe("@src/agent ");
    expect(screen.queryByTestId("mention-menu")).toBeNull();
  });

  it("Esc 仅收浮层不 blur 输入框；Tab 同 Enter 选中", async () => {
    const api = mockApi();
    render(<AgentPanel api={api} t={t} sessionId="s1" projectId="p1" />);
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    input.focus();
    fireEvent.change(input, { target: { value: "@" } });
    await screen.findByTestId("mention-menu");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByTestId("mention-menu")).toBeNull();
    expect(document.activeElement).toBe(input);
    // 再开浮层（改过滤词重触发），Tab 选中首项
    fireEvent.change(input, { target: { value: "@s" } });
    await screen.findByTestId("mention-menu");
    fireEvent.keyDown(input, { key: "Tab" });
    expect(input.value).toBe("@src/app.tsx ");
  });

  it("鼠标 mousedown 选中插入且输入框保持焦点", async () => {
    const api = mockApi();
    render(<AgentPanel api={api} t={t} sessionId="s1" projectId="p1" />);
    const input = screen.getByTestId("task-input") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "@" } });
    const item = await screen.findByTestId("mention-item-0");
    fireEvent.mouseDown(item);
    expect(input.value).toBe("@src/app.tsx ");
    expect(screen.queryByTestId("mention-menu")).toBeNull();
  });

  it("a@b 邮箱形态不弹浮层；无 projectId 不触发", async () => {
    const api = mockApi();
    const { unmount } = render(
      <AgentPanel api={api} t={t} sessionId="s1" projectId="p1" />,
    );
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "mail me a@b" } });
    await new Promise((r) => setTimeout(r, 200));
    expect(screen.queryByTestId("mention-menu")).toBeNull();
    expect((api.fuzzyFiles as ReturnType<typeof vi.fn>).mock.calls.length).toBe(0);
    unmount();

    const api2 = mockApi();
    render(<AgentPanel api={api2} t={t} sessionId="s1" />);
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "@" } });
    await new Promise((r) => setTimeout(r, 200));
    expect(screen.queryByTestId("mention-menu")).toBeNull();
    expect((api2.fuzzyFiles as ReturnType<typeof vi.fn>).mock.calls.length).toBe(0);
  });
});
