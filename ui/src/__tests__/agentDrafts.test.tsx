// 草稿持久化（§7.2 v1.177）：lib 语义 + AgentPanel 会话草稿恢复 / 发送清空。
// 本环境 jsdom 不提供 localStorage（settings.test 同款）：测试桩内存后端。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import { clearDrafts, loadDrafts, saveDrafts } from "../lib/drafts";
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

function mockApi(events: Array<Record<string, unknown>> = [], status = "idle"): TenonApi {
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
    sendMessage: vi.fn().mockResolvedValue({}),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("lib/drafts", () => {
  beforeEach(() => clearDrafts());
  afterEach(() => clearDrafts());

  it("roundtrip：save 后 load 原样恢复", () => {
    saveDrafts({ drafts: { p1: "项目草稿" }, sessions: { s1: "会话草稿" } });
    const store = loadDrafts();
    expect(store.drafts["p1"]).toBe("项目草稿");
    expect(store.sessions["s1"]).toBe("会话草稿");
  });

  it("损坏 JSON / 缺键容错回空表", () => {
    localStorage.setItem("tenon:drafts", "{not json");
    expect(loadDrafts()).toEqual({ drafts: {}, sessions: {} });
    localStorage.setItem("tenon:drafts", JSON.stringify({ drafts: "oops", sessions: 42 }));
    expect(loadDrafts()).toEqual({ drafts: {}, sessions: {} });
  });

  it("非字符串值被过滤", () => {
    localStorage.setItem(
      "tenon:drafts",
      JSON.stringify({ drafts: { p1: "ok", p2: 123 }, sessions: { s1: null } }),
    );
    const store = loadDrafts();
    expect(store.drafts).toEqual({ p1: "ok" });
    expect(store.sessions).toEqual({});
  });

  it("单条超 20k 字符不落盘；空串过滤", () => {
    saveDrafts({
      drafts: { big: "x".repeat(20_001), empty: "" },
      sessions: { s1: "ok" },
    });
    const store = loadDrafts();
    expect(store.drafts["big"]).toBeUndefined();
    expect(store.drafts["empty"]).toBeUndefined();
    expect(store.sessions["s1"]).toBe("ok");
  });

  it("每表超 50 条按写入序淘汰最旧", () => {
    const drafts: Record<string, string> = {};
    for (let i = 0; i < 55; i++) drafts[`p${i}`] = `d${i}`;
    saveDrafts({ drafts, sessions: {} });
    const store = loadDrafts();
    expect(Object.keys(store.drafts).length).toBe(50);
    expect(store.drafts["p0"]).toBeUndefined();
    expect(store.drafts["p54"]).toBe("d54");
  });
});

describe("AgentPanel 草稿持久化（v1.177 §7.2）", () => {
  beforeEach(() => clearDrafts());
  afterEach(() => clearDrafts());

  it("会话输入草稿刷新（重挂载）后恢复", async () => {
    const { unmount } = render(<AgentPanel api={mockApi()} t={t} sessionId="s-draft-x" />);
    const ta = (await screen.findByTestId("task-input")) as HTMLTextAreaElement;
    fireEvent.change(ta, { target: { value: "未发送的草稿内容" } });
    // 防抖 600ms 落盘
    await waitFor(
      () => {
        expect(localStorage.getItem("tenon:drafts")).toContain("未发送的草稿内容");
      },
      { timeout: 2000 },
    );
    unmount();
    // 模拟刷新：全新挂载同一会话
    render(<AgentPanel api={mockApi()} t={t} sessionId="s-draft-x" />);
    const ta2 = (await screen.findByTestId("task-input")) as HTMLTextAreaElement;
    expect(ta2.value).toBe("未发送的草稿内容");
  });

  it("发送成功后会话草稿清空", async () => {
    const send = vi.fn().mockResolvedValue({});
    const api = { ...mockApi(), sendMessage: send } as unknown as TenonApi;
    render(<AgentPanel api={api} t={t} sessionId="s-draft-y" />);
    const ta = (await screen.findByTestId("task-input")) as HTMLTextAreaElement;
    fireEvent.change(ta, { target: { value: "即将发送" } });
    fireEvent.click(screen.getByTestId("send"));
    await waitFor(() => expect(send).toHaveBeenCalled());
    await waitFor(
      () => {
        const store = loadDrafts();
        expect(store.sessions["s-draft-y"] ?? "").toBe("");
      },
      { timeout: 2000 },
    );
  });
});
