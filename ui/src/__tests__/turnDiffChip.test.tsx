// v1.160 回合改动摘要徽标（§7.5）覆盖：diff 统计纯函数（文件去重 / 增删行 / 头不计）/
// 徽标渲染 / 点击注入回合聚合 diff / 无改动回合不渲染。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel, turnDiffStats } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string, params?: Record<string, unknown>) =>
  params ? `${key}:${params.n}` : key;

const DIFF_A = [
  "--- a/src/app.ts",
  "+++ b/src/app.ts",
  "@@ -1,2 +1,3 @@",
  " context",
  "-old",
  "+new1",
  "+new2",
].join("\n");

const DIFF_B = [
  "--- a/src/lib.rs",
  "+++ b/src/lib.rs",
  "@@ -10,2 +10,1 @@",
  "-gone",
  " keep",
].join("\n");

function patchEv(id: number, seq: number, content: string) {
  return { id, seq, type: "patch_applied", payload: { output: { content } } };
}

describe("turnDiffStats 纯函数", () => {
  it("文件去重计数，hunk 内 +/− 行分别统计，diff 头不计", () => {
    const stats = turnDiffStats([patchEv(1, 1, DIFF_A)]);
    expect(stats).toEqual({ diff: DIFF_A, files: 1, additions: 2, deletions: 1 });
  });

  it("多 patch 拼接 diff，文件跨 patch 去重，增删跨 patch 累加", () => {
    const stats = turnDiffStats([patchEv(1, 1, DIFF_A), patchEv(2, 2, DIFF_B)]);
    expect(stats?.files).toBe(2);
    expect(stats?.additions).toBe(2);
    expect(stats?.deletions).toBe(2);
    expect(stats?.diff).toBe(`${DIFF_A}\n${DIFF_B}`);
  });

  it("无 patch_applied 或 diff 不可解析返回 null", () => {
    expect(turnDiffStats([])).toBeNull();
    expect(
      turnDiffStats([{ id: 1, seq: 1, type: "decision", payload: {} }]),
    ).toBeNull();
    expect(turnDiffStats([patchEv(1, 1, "not a diff")])).toBeNull();
  });
});

describe("AgentPanel 回合改动摘要徽标（v1.160）", () => {
  function mockApi(events: Array<Record<string, unknown>>) {
    return {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockImplementation((_sessionId: string, after = 0) =>
        Promise.resolve({
          events: events.filter((e) => (e.seq as number) > after),
          latest_seq: events.length,
        }),
      ),
      getSession: vi.fn().mockResolvedValue({
        session_id: "s1",
        status: "done",
        latest_seq: events.length,
        outcome: null,
        queue: [],
      }),
      sendMessage: vi.fn().mockResolvedValue({ accepted: true, queued: false }),
      control: vi.fn().mockResolvedValue({ ok: true }),
    } as unknown as TenonApi;
  }

  it("含改动回合渲染徽标「N 文件 · +A −B」，点击回调注入回合聚合 diff", async () => {
    const onShowDiff = vi.fn();
    const api = mockApi([
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      patchEv(2, 2, DIFF_A),
      patchEv(3, 3, DIFF_B),
    ]);
    render(
      <AgentPanel api={api} t={t} sessionId="s1" onShowDiff={onShowDiff} />,
    );
    const chip = await screen.findByTestId("turn-diffchip");
    expect(chip.textContent).toContain("thread.diffchip_files:2");
    expect(chip.textContent).toContain("+2");
    expect(chip.textContent).toContain("−2");
    fireEvent.click(chip);
    await waitFor(() =>
      expect(onShowDiff.mock.calls[0]).toEqual([`${DIFF_A}\n${DIFF_B}`]),
    );
  });

  it("无改动回合不渲染徽标", async () => {
    const api = mockApi([
      { id: 1, seq: 1, type: "user_input", payload: { text: "纯问答" } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "回答" } },
    ]);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await screen.findByTestId("turn-user");
    await waitFor(() => expect(api.getSession).toHaveBeenCalled());
    expect(screen.queryByTestId("turn-diffchip")).toBeNull();
  });
});
