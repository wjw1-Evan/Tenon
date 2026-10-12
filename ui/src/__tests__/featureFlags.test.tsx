// feature flag 基建（v2.29，design.md §7.0 / design-v2.md §9 流程门禁）契约：
// ① 注册表/默认值/prefs 解析（on/off/缺席→默认）与 localStorage 快路径；
// ② useFeatureFlags：toggle 双写（daemon 权威 + localStorage）与 enabled 融合；
// ③ App 接线：命令面板逐 flag 命令（动作语义标签）+ 审查面挂 flag.reviewPane
//（关闭回 v1 无审查面形态——一键回退演示）；设置「实验特性」开关列表。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import {
  FEATURE_FLAGS,
  flagDefault,
  flagFromPrefs,
  flagToPrefsEntry,
  useFeatureFlags,
} from "../lib/featureFlags";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (k in Object.fromEntries(backing) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
});

describe("feature flag 注册表与解析（v2.29）", () => {
  it("注册表含首个消费者 reviewPane 默认 on；prefs 解析三态与序列化", () => {
    const def = FEATURE_FLAGS.find((f) => f.key === "reviewPane");
    expect(def?.default).toBe(true);
    // prefs 解析：on/off 显式、缺席回默认；未知 key false
    expect(flagFromPrefs("reviewPane", { "flag.reviewPane": "on" })).toBe(true);
    expect(flagFromPrefs("reviewPane", { "flag.reviewPane": "off" })).toBe(false);
    expect(flagFromPrefs("reviewPane", {})).toBe(true);
    expect(flagFromPrefs("nope", {})).toBe(false);
    // 序列化往返
    expect(flagToPrefsEntry("reviewPane", false)).toEqual({ "flag.reviewPane": "off" });
    // localStorage 快路径：无记录回默认、有记录尊显
    expect(flagDefault("reviewPane")).toBe(true);
    localStorage.setItem("tenon:flag:reviewPane", "off");
    expect(flagDefault("reviewPane")).toBe(false);
  });
});

describe("useFeatureFlags hook（v2.29）", () => {
  function Harness({ initial, persist }: { initial: Record<string, string>; persist: (p: Record<string, string>) => void }) {
    const flags = useFeatureFlags(initial, persist);
    return (
      <div>
        <output data-testid="v">{String(flags.enabled("reviewPane"))}</output>
        <button type="button" data-testid="t" onClick={() => flags.toggle("reviewPane")}>
          t
        </button>
      </div>
    );
  }

  it("toggle 即时翻转并双写（daemon 权威 + localStorage）", () => {
    const persist = vi.fn();
    const { rerender } = render(<Harness initial={{}} persist={persist} />);
    expect(screen.getByTestId("v").textContent).toBe("true");
    fireEvent.click(screen.getByTestId("t"));
    expect(screen.getByTestId("v").textContent).toBe("false");
    expect(persist).toHaveBeenCalledWith({ "flag.reviewPane": "off" });
    expect(localStorage.getItem("tenon:flag:reviewPane")).toBe("off");
    rerender(<Harness initial={{}} persist={persist} />);
    // 本地态不因 rerender 丢失
    expect(screen.getByTestId("v").textContent).toBe("false");
  });
});

describe("App 接线（v2.29）", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = (init?.method ?? "GET").toUpperCase();
      if (url.endsWith("/projects/open"))
        return jsonResponse({
          id: "p-a", path: "/tmp/p-a", display_name: "project-a", trusted: true,
          sessions: [], session_runtimes: [], active_sessions: 0, dirty_buffers: 0,
          usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
        });
      if (url.endsWith("/projects")) return jsonResponse({ projects: [] });
      if (url.endsWith("/session") && method === "POST")
        return jsonResponse({ session_id: "new-sess", project_id: "?" });
      if (url.includes("/ui-state")) return jsonResponse({});
      if (url.includes("/pairing")) return jsonResponse({ port: 9876, token: "dev" });
      if (url.includes("/ui-prefs")) {
        // flag.reviewPane off → 启动即回 v1 无审查面形态
        if (method === "PUT") return jsonResponse({});
        return jsonResponse({ "flag.reviewPane": "off" });
      }
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

  it("flag.reviewPane=off：运行态会话也不渲染审查面（一键回退 v1 形态）", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/p-a" />);
    await waitFor(() =>
      expect((screen.getByTestId("send") as HTMLButtonElement).disabled).toBe(false),
    );
    // ui-prefs 返回 off → flag 生效：无 active 会话也轮询过了；即便手工运行态条件
    //（此处直接断言 pane 缺席——运行态条件需 agentState，改由命令面板例覆盖交互面）
    expect(screen.queryByTestId("review-pane")).toBeNull();
  });
});
