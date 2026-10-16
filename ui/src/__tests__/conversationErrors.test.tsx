// v2.89 对话错误友好化与自愈（§6.2 / §9.1 / §9.4）：
//   - daemon 重启中断（interrupted）渲染为可续跑的中性提示，不再是错误；
//   - 模型错误按 daemon 分类载荷渲染友好文案 + 可行动 hint；
//   - 回滚失败（rollback_failed）给出明确原因，不再落「未知错误」兜底；
//   - 验证失败自动修复轮渲染「自动修复中」提示卡。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const t = (key: string) => key;

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
});
afterEach(() => vi.unstubAllGlobals());

function mockApi(events: Array<Record<string, unknown>>): TenonApi {
  return {
    models: vi.fn().mockResolvedValue({ models: [], default: "", laya: null }),
    trace: vi.fn().mockImplementation((_sessionId: string, after = 0) =>
      Promise.resolve({
        events: events.filter((e) => (e.seq as number) > after),
        latest_seq: events.length,
      }),
    ),
    getSession: vi
      .fn()
      .mockResolvedValue({ session_id: "s1", status: "done", latest_seq: events.length, outcome: null }),
    sendMessage: vi.fn(),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("对话错误友好化（v2.89）", () => {
  it("interrupted 渲染为可续跑提示，不出现错误样式", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "跑个任务" } },
      {
        id: 2,
        seq: 2,
        type: "error",
        payload: { interrupted: "daemon 重启中断——上下文已保留，发送消息即新 Run 续跑" },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    expect(await screen.findByTestId("turn-interrupted")).toBeTruthy();
    expect(screen.queryByTestId("turn-error")).toBeNull();
  });

  it("模型鉴权错误渲染友好文案 + 可行动 hint", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "跑个任务" } },
      {
        id: 2,
        seq: 2,
        type: "error",
        payload: {
          error: "模型鉴权失败（HTTP 401）",
          code: "model_auth",
          hint: "API Key 可能无效或已过期，请在设置 → 模型中更新",
          provider: "glm",
          detail: "HTTP 错误 401: {...}",
        },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const err = await screen.findByTestId("turn-error");
    expect(err.textContent).toContain("模型鉴权失败");
    expect(err.querySelector(".turn-error-hint")?.textContent).toContain("API Key");
  });

  it("rollback_failed 渲染明确原因，不再落「未知错误」", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "跑个任务" } },
      {
        id: 2,
        seq: 2,
        type: "error",
        payload: { rollback_failed: "object not found", intended_tree: "abc123" },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const err = await screen.findByTestId("turn-error");
    expect(err.textContent).toContain("thread.rollback_failed");
    expect(err.textContent).not.toContain("agent.error_fallback");
  });

  it("验证失败自动修复轮渲染提示卡", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "跑个任务" } },
      {
        id: 2,
        seq: 2,
        type: "diagnostics",
        payload: { fixing: true, round: 1, of: 3, verification: "$ npm test\nexit=1" },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const note = await screen.findByTestId("turn-fixing");
    expect(note.textContent).toContain("thread.fixing");
  });
});
