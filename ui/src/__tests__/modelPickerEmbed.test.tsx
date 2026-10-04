// v1.51：模型选择入口内嵌代理面板任务输入框——pill 位于输入区底行、发送按钮右下，
// 对话框交互（清单 / 高亮 / 会话级切换）在嵌入位置可用。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import type { TenonApi } from "../lib/api";

const models = {
  models: [
    { name: "glm", default_model: "glm-5.3-flash", local: false, is_default: true },
    { name: "ollama", default_model: "qwen3:8b", local: true, is_default: false },
  ],
  default: "glm",
  laya: null,
};

function makeApi() {
  return {
    models: vi.fn().mockResolvedValue(models),
    modelSuggest: vi.fn().mockResolvedValue({ pure_read: true, source: {} }),
    switchSessionModel: vi.fn().mockResolvedValue({ model: "ollama" }),
    sendMessage: vi.fn().mockResolvedValue({ accepted: true }),
    trace: vi.fn().mockResolvedValue({ events: [] }),
    getSession: vi
      .fn()
      .mockResolvedValue({ session_id: "s1", status: "idle", latest_seq: 0, outcome: null }),
  } as unknown as TenonApi;
}

const t = (k: string) => k;

describe("模型选择内嵌任务输入框（v1.51）", () => {
  it("输入区底行渲染模型 pill 与发送按钮，pill 显示当前模型", async () => {
    const { container } = render(<AgentPanel api={makeApi()} t={t} sessionId="s1" />);
    const trigger = await waitFor(() => {
      const el = container.querySelector<HTMLElement>(
        ".agent-input-foot [data-testid='model-trigger']"
      );
      expect(el).not.toBeNull();
      return el!;
    });
    expect(trigger.textContent).toContain("glm");
    const foot = trigger.closest(".agent-input-foot")!;
    expect(foot).not.toBeNull();
    expect(foot.querySelector("[data-testid='send']")).not.toBeNull();
    // 组合容器：textarea 与底行同在 .agent-input 内
    expect(
      trigger.closest(".agent-input")?.querySelector("[data-testid='task-input']")
    ).not.toBeNull();
  });

  it("从输入框内 pill 打开对话框并完成会话级切换", async () => {
    const api = makeApi();
    const onModelSwitched = vi.fn();
    render(<AgentPanel api={api} t={t} sessionId="s1" onModelSwitched={onModelSwitched} />);
    fireEvent.click(await screen.findByTestId("model-trigger"));
    const option = await screen.findByTestId("model-option-ollama");
    expect(option.textContent).toContain("model.local_free");
    fireEvent.click(option);
    await waitFor(() => expect(onModelSwitched).toHaveBeenCalledWith("ollama"));
    expect(api.switchSessionModel).toHaveBeenCalledWith("s1", "ollama");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
