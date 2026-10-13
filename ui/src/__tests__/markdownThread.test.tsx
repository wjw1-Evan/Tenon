// v2.59 对话内容渲染质量（组件级）：代码块复制事件委托 / 表格渲染 / 流式光标类挂载。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(events: Array<Record<string, unknown>>, status = "idle") {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events, latest_seq: events.length }),
    getSession: vi.fn().mockResolvedValue({
      session_id: "s1",
      status,
      latest_seq: events.length,
      outcome: null,
    }),
    sendMessage: vi.fn().mockResolvedValue({}),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("回合正文渲染（v2.59）", () => {
  it("decision 正文中的表格与代码块按 markdown 渲染", async () => {
    const md = [
      "| 命令 | 说明 |",
      "| --- | --- |",
      "| `cargo test` | 跑测试 |",
      "",
      "```rust",
      "let x = 1;",
      "```",
    ].join("\n");
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "decision", payload: { intent: md } },
    ];
    const { container } = render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText("任务");
    await waitFor(() => expect(container.querySelector(".turn-answer table")).toBeTruthy());
    expect(container.querySelector(".code-lang")?.textContent).toBe("rust");
    expect(container.querySelector(".code-copy")?.textContent).toBe("thread.code_copy");
  });

  it("点击代码块复制钮：事件委托取同块 pre 文本写剪贴板，按钮瞬时 ✓ 反馈", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      {
        id: 2,
        seq: 2,
        type: "decision",
        payload: { intent: "```py\nprint(1)\n```" },
      },
    ];
    const { container } = render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const btn = (await waitFor(() => {
      const el = container.querySelector(".code-copy");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    })) as HTMLElement;
    fireEvent.click(btn);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("print(1)"));
    expect(btn.textContent).toBe("✓");
    vi.unstubAllGlobals();
  });
});

describe("流式光标（v2.59）", () => {
  it("运行态有流式正文挂 live-stream 类；无流式文本（spinner 行承接）不挂", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "model_delta", payload: { text: "部分回答" } },
    ];
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockResolvedValue({ events, latest_seq: 2 }),
      getSession: vi.fn().mockResolvedValue({
        session_id: "s1",
        status: "deciding",
        latest_seq: 2,
        outcome: null,
      }),
      sendMessage: vi.fn().mockResolvedValue({}),
      control: vi.fn().mockResolvedValue({ ok: true }),
    } as unknown as TenonApi;
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const stream = await screen.findByTestId("model-stream");
    expect(stream).toHaveTextContent("部分回答");
    // 运行态（deciding ∈ RUNNING_STATES）+ 流式文本 → live-stream（::after 光标）
    expect(stream.className).toContain("live-stream");
  });
});
