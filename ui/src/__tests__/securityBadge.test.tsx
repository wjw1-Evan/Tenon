// v2.17 顶栏安全姿态指示（design-v2.md §4.6）契约：
// ① SecurityBadge：三色 tone / 缩写与 data 属性 / title 全名 / 点击回调；
// ② App 接线：会话档上提后徽标渲染（无会话不渲染）、点击直达设置 Permissions 节
//    且默认档两 select 在位、保存载荷携带 security；
// ③ SettingsDialog：security 默认档回显 + 保存进 PUT 载荷。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { SecurityBadge } from "../components/SecurityBadge";
import { SettingsDialog } from "../components/SettingsDialog";
import type { ProjectSummary, SettingsData } from "../lib/api";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (k in Object.fromEntries(backing) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
});

const t = (k: string, vars?: Record<string, string | number>) => {
  let s = k;
  if (vars) for (const [kk, vv] of Object.entries(vars)) s = s.replaceAll(`{${kk}}`, String(vv));
  return s;
};

describe("SecurityBadge 组件（v2.17）", () => {
  it("风险三色与缩写：read_only 绿 RO、full_access 红 FA、workspace_write 琥珀 WW · OI", () => {
    const { rerender } = render(
      <SecurityBadge t={t} gate={{ exec_mode: "read_only", approval: "never" }} onClick={() => {}} />
    );
    let badge = screen.getByTestId("security-badge");
    expect(badge.className).toContain("tone-safe");
    expect(badge.getAttribute("data-exec-mode")).toBe("read_only");
    expect(badge.textContent).toContain("RO");
    rerender(
      <SecurityBadge t={t} gate={{ exec_mode: "full_access", approval: "always" }} onClick={() => {}} />
    );
    badge = screen.getByTestId("security-badge");
    expect(badge.className).toContain("tone-risky");
    expect(badge.textContent).toContain("FA");
    rerender(
      <SecurityBadge
        t={t}
        gate={{ exec_mode: "workspace_write", approval: "on_irreversible" }}
        onClick={() => {}}
      />
    );
    badge = screen.getByTestId("security-badge");
    expect(badge.className).toContain("tone-caution");
    expect(badge.textContent).toContain("WW · OI");
    expect(badge.getAttribute("title")).toContain("gates.exec_workspace_write");
  });

  it("点击回调（直达设置 Permissions）", () => {
    const onClick = vi.fn();
    render(
      <SecurityBadge
        t={t}
        gate={{ exec_mode: "workspace_write", approval: "on_irreversible" }}
        onClick={onClick}
      />
    );
    fireEvent.click(screen.getByTestId("security-badge"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

describe("SettingsDialog security 默认档（v2.17）", () => {
  function makeApi(put = vi.fn().mockResolvedValue({ session: {} })) {
    return {
      putSettings: put,
      putTeamPolicy: vi.fn().mockResolvedValue({}),
      listPlugins: vi.fn().mockResolvedValue({ installed: [] }),
      models: vi.fn().mockResolvedValue({ models: [], default: "", laya: null }),
    } as unknown as Parameters<typeof SettingsDialog>[0]["api"];
  }
  const baseSettings: SettingsData = {
    session: {},
    security: { exec_mode: "read_only", approval: "always" },
  };

  it("Permissions 分区：initialSection 直达、回显默认档、保存进 PUT security 载荷", async () => {
    const put = vi.fn().mockResolvedValue({ session: {} });
    render(
      <SettingsDialog
        api={makeApi(put)}
        t={t}
        settings={baseSettings}
        saveMode="auto"
        onSaveModeChange={() => {}}
        onClose={() => {}}
        onSaved={() => {}}
        initialSection="permissions"
      />
    );
    // initialSection 直达 Permissions：security 两 select 可见且回显 settings 值
    const exec = (await screen.findByTestId("settings-security-exec")) as HTMLSelectElement;
    expect(exec.value).toBe("read_only");
    const approval = screen.getByTestId("settings-security-approval") as HTMLSelectElement;
    expect(approval.value).toBe("always");
    // 改档保存 → PUT 载荷携带 security
    fireEvent.change(exec, { target: { value: "workspace_write" } });
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(put).toHaveBeenCalled());
    expect(put.mock.calls[0][0].security).toEqual({
      exec_mode: "workspace_write",
      approval: "always",
    });
  });

  it("缺省 security 块回退默认档 workspace_write × on_irreversible", async () => {
    render(
      <SettingsDialog
        api={makeApi()}
        t={t}
        settings={{ session: {} }}
        saveMode="auto"
        onSaveModeChange={() => {}}
        onClose={() => {}}
        onSaved={() => {}}
        initialSection="permissions"
      />
    );
    const exec = (await screen.findByTestId("settings-security-exec")) as HTMLSelectElement;
    expect(exec.value).toBe("workspace_write");
  });
});

describe("顶栏徽标 App 接线（v2.17）", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/projects/open"))
        return jsonResponse({
          id: "p-a",
          path: "/tmp/p-a",
          display_name: "project-a",
          trusted: true,
          sessions: [
            { id: "s-a", status: "executing", model: "mock", updated_at: "2026-01-01T00:00:00Z" },
          ],
          session_runtimes: ["s-a"],
          active_sessions: 1,
          dirty_buffers: 0,
          usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
        } as ProjectSummary);
      if (url.endsWith("/projects"))
        return jsonResponse({
          projects: [
            {
              id: "p-a",
              path: "/tmp/p-a",
              display_name: "project-a",
              trusted: true,
              sessions: [
                { id: "s-a", status: "executing", model: "mock", updated_at: "2026-01-01T00:00:00Z" },
              ],
              session_runtimes: ["s-a"],
              active_sessions: 1,
              dirty_buffers: 0,
              usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
            },
          ],
        });
      if (url.includes("/session/s-a/trace")) return jsonResponse({ events: [] });
      if (/\/session\/s-a$/.test(url))
        return jsonResponse({
          session_id: "s-a",
          status: "executing",
          latest_seq: 0,
          outcome: null,
          gate: { exec_mode: "full_access", approval: "never" },
        });
      if (url.includes("/ui-state")) return jsonResponse({});
      if (url.includes("/pairing")) return jsonResponse({ port: 9876, token: "dev" });
      if (url.includes("/ui-prefs")) return jsonResponse({});
      if (url.includes("/settings")) return jsonResponse({});
      if (url.includes("/team-policy")) return jsonResponse({ denied_tools: [], max_cost_usd: null });
      if (url.includes("/updates")) return jsonResponse({ current_version: "0.1.0", staged: null });
      if (url.includes("/models"))
        return jsonResponse({ models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
      if (url.includes("/evals")) return jsonResponse({ runs: [] });
      if (url.includes("/costs")) return jsonResponse({ input_tokens: 0, output_tokens: 0, cost_usd: 0 });
      return jsonResponse({});
    });
    vi.stubGlobal("fetch", fetchMock);
    class FakeWS {
      onopen: (() => void) | null = null;
      onmessage: ((m: { data: string }) => void) | null = null;
      onclose: (() => void) | null = null;
      onerror: (() => void) | null = null;
      constructor(_url: string) {
        queueMicrotask(() => this.onopen?.());
      }
      send(_data: string) {
        queueMicrotask(() => this.onmessage?.({ data: "auth ok" }));
      }
      close() {}
    }
    vi.stubGlobal("WebSocket", FakeWS);
  });

  function jsonResponse(body: unknown, status = 200) {
    return Promise.resolve(new Response(JSON.stringify(body), { status }));
  }

  it("会话档上提后徽标渲染（gate 载荷 full_access→红）；点击直达设置 Permissions", { timeout: 20_000 }, async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/p-a" />);
    const badge = await waitFor(
      () => {
        const node = screen.getByTestId("security-badge");
        expect(node).toBeTruthy();
        return node;
      },
      { timeout: 18_000 }
    );
    expect(badge.className).toContain("tone-risky");
    expect(badge.getAttribute("data-approval")).toBe("never");
    // 点击直达设置 Permissions 节：security 默认档 select 可见
    fireEvent.click(badge);
    await waitFor(() =>
      expect(screen.getByTestId("settings-security-exec")).toBeTruthy()
    );
  });
});
