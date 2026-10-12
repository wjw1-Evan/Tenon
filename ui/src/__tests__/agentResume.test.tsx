// v2.35 恢复双语义 + 错误原因细分（§7.5 / §9.1）：
// ① paused 恢复钮点击 → control resume 附带本地化继续语（终态暂停由 daemon 转续跑）；
// ② 恢复请求失败显式 toast 上报（此前静默，点击无响应无提示）；
// ③ 守卫类错误事件按载荷键渲染具体原因，不再裸显「错误」兜底。
// t 以真实 zh 语言表驱动（含 {var} 插值）——同时验证中文文案本身。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import { ToastHost } from "../components/ToastHost";
import type { TenonApi } from "../lib/api";
import zh from "../locales/zh.json";

const table = zh as Record<string, string>;
const t = (key: string, vars?: Record<string, string | number>) => {
  let text = table[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      text = text.replaceAll(`{${k}}`, String(v));
    }
  }
  return text;
};

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

function mockApi(
  events: Array<Record<string, unknown>>,
  status: string,
  control: ReturnType<typeof vi.fn>,
): TenonApi {
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
      .mockResolvedValue({ session_id: "s1", status, latest_seq: events.length, outcome: null }),
    control,
  } as unknown as TenonApi;
}

describe("恢复双语义（v2.35 §9.1）", () => {
  it("paused 恢复钮点击 → control resume 附带本地化继续语", async () => {
    const control = vi.fn().mockResolvedValue({ ok: true });
    render(<AgentPanel api={mockApi([], "paused", control)} t={t} sessionId="s1" />);
    const btn = await screen.findByTestId("resume");
    expect(btn.textContent).toBe("恢复");
    fireEvent.click(btn);
    await waitFor(() => expect(control).toHaveBeenCalled());
    expect(control.mock.calls[0][0]).toBe("s1");
    expect(control.mock.calls[0][1]).toBe("resume");
    expect(control.mock.calls[0][2]).toBeUndefined();
    expect(control.mock.calls[0][3]).toBe("继续");
  });

  it("恢复请求失败 → toast 显式上报（不再静默无响应）", async () => {
    const control = vi.fn().mockRejectedValue(new Error("daemon 不可达"));
    render(
      <>
        <AgentPanel api={mockApi([], "paused", control)} t={t} sessionId="s1" />
        <ToastHost t={t} />
      </>,
    );
    fireEvent.click(await screen.findByTestId("resume"));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("daemon 不可达"),
    );
  });
});

describe("错误原因细分（v2.35 §7.5）", () => {
  const cases: Array<[Record<string, unknown>, string, string]> = [
    [{ circuit_tripped: "max_files" }, "熔断器触发", "max_files"],
    [{ rounds_exhausted: 24 }, "回合上限", "24"],
    [{ snapshot_unavailable: "db locked" }, "快照库不可用", "db locked"],
  ];
  it.each(cases)("守卫类错误事件渲染具体原因（%j）", async (payload, fragment, detail) => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "error", payload },
    ];
    render(
      <AgentPanel
        api={mockApi(events, "paused", vi.fn().mockResolvedValue({ ok: true }))}
        t={t}
        sessionId="s1"
      />,
    );
    const card = await screen.findByTestId("turn-error");
    expect(card.textContent).toContain(fragment);
    expect(card.textContent).toContain(detail);
  });

  it("error 键仍直显上游原文", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "error", payload: { error: "HTTP 500: 上游故障" } },
    ];
    render(
      <AgentPanel
        api={mockApi(events, "error", vi.fn().mockResolvedValue({ ok: true }))}
        t={t}
        sessionId="s1"
      />,
    );
    const card = await screen.findByTestId("turn-error");
    expect(card.textContent).toContain("HTTP 500: 上游故障");
  });
});
