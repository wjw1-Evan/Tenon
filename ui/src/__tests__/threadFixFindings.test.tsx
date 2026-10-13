// v2.66 审查发现一键修复（§7.5）：审查回合识别（任务文本恰为审查模板）→
// 气泡下「修复以上发现」钮 → 点击发送自包含修复任务（模板 + 审查回复全文——
// daemon 每任务全新对话，指代上文模型不可见）；非审查回合不渲染；运行态禁用。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(events: unknown[], status = "idle") {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events, latest_seq: events.length }),
    getSession: vi.fn().mockResolvedValue({
      session_id: "s1",
      status,
      latest_seq: events.length,
      outcome: null,
      queue: [],
    }),
    sendMessage: vi.fn().mockResolvedValue({ accepted: true, queued: false }),
    control: vi.fn().mockResolvedValue({ ok: true }),
    fuzzyFiles: vi.fn().mockResolvedValue({ hits: [], query: "" }),
  } as unknown as TenonApi;
}

const reviewTask = "slash.review_task";

describe("审查发现一键修复（v2.66）", () => {
  it("审查回合渲染修复钮；点击发送模板 + 审查回复全文的自包含任务", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: reviewTask } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "1. 缺少空值检查（a.rs:12）\n2. 错误未处理（b.rs:30）" } },
    ];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const btn = await screen.findByTestId("turn-fix-1");
    fireEvent.click(btn);
    await waitFor(() =>
      expect((api.sendMessage as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
        "s1",
        "slash.fix_findings_task\n\n1. 缺少空值检查（a.rs:12）\n2. 错误未处理（b.rs:30）",
      ]),
    );
  });

  it("非审查回合不渲染修复钮", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "普通任务" } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "完成" } },
    ];
    const api = mockApi(events);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByText("普通任务");
    expect(screen.queryByTestId("turn-fix-1")).toBeNull();
  });

  it("运行态修复钮禁用", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: reviewTask } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "发现若干问题" } },
    ];
    const api = mockApi(events, "executing");
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const btn = await screen.findByTestId("turn-fix-1") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });
});
