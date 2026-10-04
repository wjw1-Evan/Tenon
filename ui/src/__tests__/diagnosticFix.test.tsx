// 诊断面板的 AI 修复必须直接注入当前项目会话（附录 D T4）。
import { render, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

describe("AgentPanel diagnostic task injection", () => {
  it("sends injected diagnostic instructions to the active session", async () => {
    const sendMessage = vi.fn().mockResolvedValue({ accepted: true });
    const api = {
      trace: vi.fn().mockResolvedValue({ events: [], latest_seq: 0 }),
      getSession: vi.fn().mockResolvedValue({ status: "idle", latest_seq: 0, outcome: null }),
      sendMessage,
      // v1.51：AgentPanel 输入框内嵌模型选择器，挂载即拉取 GET /models。
      models: vi.fn().mockResolvedValue({ models: [], default: "", laya: null }),
    } as unknown as TenonApi;

    render(
      <AgentPanel
        api={api}
        t={(key) => key}
        sessionId="session-1"
        injectedTask={{ token: 1, text: "修复 src/app.ts:12 的诊断。" }}
      />
    );

    await waitFor(() => expect(sendMessage).toHaveBeenCalledWith(
      "session-1",
      "修复 src/app.ts:12 的诊断。"
    ));
  });
});
