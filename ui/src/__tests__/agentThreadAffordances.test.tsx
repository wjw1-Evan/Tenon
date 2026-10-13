// v2.58 对话交互三件套：回底浮钮显隐与未读徽点 / 用户回合「编辑」回填 / 输入框随内容自增高。
// jsdom 无布局引擎——滚动几何（scrollHeight/clientHeight/scrollTop）以实例属性覆写注入。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function baseApi(trace: TenonApi["trace"], status = "idle") {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace,
    getSession: vi.fn().mockResolvedValue({ session_id: "s1", status, latest_seq: 2, outcome: null }),
    sendMessage: vi.fn().mockResolvedValue({}),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

const twoTurnEvents = [
  { id: 1, seq: 1, type: "user_input", payload: { text: "任务一" } },
  { id: 2, seq: 2, type: "decision", payload: { intent: "答案一" } },
];

function setFeedMetrics(scrollTop: number): HTMLElement {
  const feed = screen.getByTestId("agent-feed");
  Object.defineProperty(feed, "scrollHeight", { value: 1000, configurable: true });
  Object.defineProperty(feed, "clientHeight", { value: 300, configurable: true });
  Object.defineProperty(feed, "scrollTop", { value: scrollTop, configurable: true, writable: true });
  return feed;
}

describe("回底浮钮（v2.58 三件套①）", () => {
  it("距底 ≥160px 显现浮钮；离底期间事件流继续增长出未读徽点；点击回底复位", async () => {
    let call = 0;
    const trace = vi.fn().mockImplementation(() => {
      call += 1;
      if (call === 1)
        return Promise.resolve({ events: twoTurnEvents, latest_seq: 2 });
      // 轮询按 after 游标增量拉取：后续只回新事件（全量重放会重复键）
      return Promise.resolve({
        events: [{ id: 3, seq: 3, type: "user_input", payload: { text: "任务二" } }],
        latest_seq: 3,
      });
    });
    render(<AgentPanel api={baseApi(trace)} t={t} sessionId="s1" />);
    await screen.findByText("任务一");
    // jsdom 无布局：默认几何 gap=0 <160 恒粘底，无浮钮
    expect(screen.queryByTestId("agent-jump")).toBeNull();
    // 用户上滑离底（scrollHeight 1000 / clientHeight 300 / scrollTop 0 → gap 700）
    const feed = setFeedMetrics(0);
    fireEvent.scroll(feed);
    expect(screen.getByTestId("agent-jump")).toBeTruthy();
    expect(screen.queryByTestId("agent-jump-dot")).toBeNull();
    // 离底期间事件流继续增长（500ms 轮询第 2 拉新事件）→ 未读徽点
    await waitFor(() => expect(screen.getByTestId("agent-jump-dot")).toBeTruthy(), {
      timeout: 3000,
    });
    // 点击回底：滚回底部 + 浮钮与徽点复位
    fireEvent.click(screen.getByTestId("agent-jump"));
    expect(feed.scrollTop).toBe(1000);
    expect(screen.queryByTestId("agent-jump")).toBeNull();
  });

  it("手动滚回阈值内（gap <160px）浮钮自动隐藏", async () => {
    const trace = vi.fn().mockResolvedValue({ events: twoTurnEvents, latest_seq: 2 });
    render(<AgentPanel api={baseApi(trace)} t={t} sessionId="s1" />);
    await screen.findByText("任务一");
    const feed = setFeedMetrics(0);
    fireEvent.scroll(feed);
    expect(screen.getByTestId("agent-jump")).toBeTruthy();
    // 滚回近底：gap = 1000 - 700 - 300 = 0 <160
    feed.scrollTop = 700;
    fireEvent.scroll(feed);
    expect(screen.queryByTestId("agent-jump")).toBeNull();
  });
});

describe("用户回合「编辑」（v2.58 三件套③）", () => {
  it("点击编辑：任务文本回填输入框并聚焦（同队列气泡回填语义）", async () => {
    const trace = vi.fn().mockResolvedValue({ events: twoTurnEvents, latest_seq: 2 });
    render(<AgentPanel api={baseApi(trace)} t={t} sessionId="s1" />);
    await screen.findByText("任务一");
    const editBtn = document.querySelector('[data-testid^="turn-edit-"]')!;
    expect(editBtn).toBeTruthy();
    fireEvent.click(editBtn);
    const ta = screen.getByTestId("task-input") as HTMLTextAreaElement;
    expect(ta.value).toBe("任务一");
    expect(document.activeElement).toBe(ta);
  });
});

describe("输入框自增高（v2.58 三件套②）", () => {
  it("随内容逐行增高至 180px 封顶，内容变短跟随回缩", async () => {
    const trace = vi.fn().mockResolvedValue({ events: [], latest_seq: 0 });
    render(<AgentPanel api={baseApi(trace)} t={t} sessionId="s1" />);
    const ta = (await screen.findByTestId("task-input")) as HTMLTextAreaElement;
    Object.defineProperty(ta, "scrollHeight", { value: 460, configurable: true });
    fireEvent.change(ta, { target: { value: "很长\n很长\n很长\n的内容" } });
    expect(ta.style.height).toBe("180px");
    Object.defineProperty(ta, "scrollHeight", { value: 80, configurable: true });
    fireEvent.change(ta, { target: { value: "短" } });
    expect(ta.style.height).toBe("80px");
  });
});
