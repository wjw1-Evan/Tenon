// v2.0 安全档位（design-v2.md §4.1）UI 契约：
// ① 会话态上下文条渲染执行边界 / 确认策略两个切换器，切换即时发 control 命令；
// ② awaiting_confirm 状态 + pending_confirm 载荷 → 确认卡（工具 / 级别 / 参数
//    预览 + 三键决议），点击调 POST /session/:id/confirm。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const controlGearCalls: Array<{ action: string; gear: string }> = [];
const confirmCalls: string[] = [];

beforeEach(() => {
  controlGearCalls.length = 0;
  confirmCalls.length = 0;
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (k in Object.fromEntries(backing) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
});

function gateApi(overrides: Partial<{ status: string; pending: unknown }> = {}) {
  return {
    models: () => Promise.resolve({ models: [], default: "", laya: null }),
    trace: () => Promise.resolve({ events: [] }),
    getSession: () =>
      Promise.resolve({
        session_id: "s1",
        status: overrides.status ?? "idle",
        latest_seq: 0,
        outcome: null,
        queue: [],
        pending_confirm: overrides.pending ?? null,
        gate: { exec_mode: "workspace_write", approval: "on_irreversible" },
      }),
    sendMessage: () => Promise.resolve({ accepted: true }),
    controlGear: (sessionId: string, action: "set_approval" | "set_exec_mode", gear: string) => {
      controlGearCalls.push({ action, gear });
      return Promise.resolve({ ok: true });
    },
    resolveConfirm: (sessionId: string, decision: string) => {
      confirmCalls.push(decision);
      return Promise.resolve({ resolved: true });
    },
  } as unknown as TenonApi;
}

const PROJECTS = [
  {
    id: "proj-a",
    path: "/tmp/proj-a",
    display_name: "project-a",
    trusted: true,
    sessions: [],
    active_sessions: 0,
    dirty_buffers: 0,
    usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
  },
];

function renderSession(api: TenonApi) {
  return render(
    <AgentPanel api={api} t={(k) => k} sessionId="s1" projectId="proj-a" projects={PROJECTS} />
  );
}

describe("v2.0 档位切换器（design-v2.md §4.1 / §6）", () => {
  it("session state renders exec/approval selects with gate snapshot values", async () => {
    renderSession(gateApi());
    await waitFor(() => {
      const exec = screen.getByTestId("gate-exec-select") as HTMLSelectElement;
      expect(exec.value).toBe("workspace_write");
    });
    const approval = screen.getByTestId("gate-approval-select") as HTMLSelectElement;
    expect(approval.value).toBe("on_irreversible");
  });

  it("switching approval fires controlGear set_approval", async () => {
    renderSession(gateApi());
    await waitFor(() => expect(screen.getByTestId("gate-approval-select")).toBeTruthy());
    fireEvent.change(screen.getByTestId("gate-approval-select"), {
      target: { value: "never" },
    });
    await waitFor(() =>
      expect(controlGearCalls).toContainEqual({ action: "set_approval", gear: "never" })
    );
    expect((screen.getByTestId("gate-approval-select") as HTMLSelectElement).value).toBe(
      "never"
    );
  });

  it("switching exec mode fires controlGear set_exec_mode", async () => {
    renderSession(gateApi());
    await waitFor(() => expect(screen.getByTestId("gate-exec-select")).toBeTruthy());
    fireEvent.change(screen.getByTestId("gate-exec-select"), {
      target: { value: "full_access" },
    });
    await waitFor(() =>
      expect(controlGearCalls).toContainEqual({ action: "set_exec_mode", gear: "full_access" })
    );
  });

  it("draft state hides gate selects (session-level switching only)", () => {
    render(
      <AgentPanel
        api={gateApi()}
        t={(k) => k}
        sessionId={null}
        draft
        projectId="proj-a"
        projects={PROJECTS}
      />
    );
    expect(screen.queryByTestId("gate-exec-select")).toBeNull();
    expect(screen.queryByTestId("gate-approval-select")).toBeNull();
  });
});

describe("v2.0 档位确认卡（design-v2.md §4.1）", () => {
  it("awaiting_confirm renders card with tool/level/args and three decisions", async () => {
    renderSession(
      gateApi({
        status: "awaiting_confirm",
        pending: {
          tool: "git_commit",
          level: "d",
          args: '{"message": "release v2"}',
        },
      })
    );
    await waitFor(() => expect(screen.getByTestId("confirm-card")).toBeTruthy());
    expect(screen.getByTestId("confirm-tool").textContent).toBe("git_commit");
    expect(screen.getByTestId("confirm-level").textContent).toBe("D");
    expect(screen.getByTestId("confirm-allow-once")).toBeTruthy();
    expect(screen.getByTestId("confirm-allow-session")).toBeTruthy();
    expect(screen.getByTestId("confirm-deny")).toBeTruthy();
  });

  it("clicking deny resolves confirm via API and clears the card", async () => {
    const { rerender } = renderSession(
      gateApi({
        status: "awaiting_confirm",
        pending: { tool: "git_push", level: "d", args: "{}" },
      })
    );
    await waitFor(() => expect(screen.getByTestId("confirm-card")).toBeTruthy());
    fireEvent.click(screen.getByTestId("confirm-deny"));
    await waitFor(() => expect(confirmCalls).toEqual(["deny"]));
    // 决议后卡片收起（乐观清除，状态轮询兜底）
    await waitFor(() => expect(screen.queryByTestId("confirm-card")).toBeNull());
    rerender(
      <AgentPanel
        api={gateApi({ status: "idle" })}
        t={(k) => k}
        sessionId="s1"
        projectId="proj-a"
        projects={PROJECTS}
      />
    );
    expect(screen.queryByTestId("confirm-card")).toBeNull();
  });

  it("idle sessions never render the card even with stale pending payload", () => {
    renderSession(
      gateApi({
        status: "done",
        pending: { tool: "git_commit", level: "d", args: "{}" },
      })
    );
    expect(screen.queryByTestId("confirm-card")).toBeNull();
  });
});
