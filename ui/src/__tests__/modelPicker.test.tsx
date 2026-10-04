// 模型选择对话框（§7.2 / §11 v1.46）：按钮触发、清单高亮、会话级切换、Esc/遮罩关闭。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ModelRoutingPanel } from "../components/ModelRoutingPanel";
import type { TenonApi } from "../lib/api";

const models = {
  models: [
    { name: "glm", default_model: "glm-5.3-flash", local: false, is_default: true },
    { name: "ollama", default_model: "qwen3:8b", local: true, is_default: false },
  ],
  default: "glm",
  laya: null,
};

function makeApi(opts: { switchModel?: ReturnType<typeof vi.fn> } = {}) {
  return {
    models: vi.fn().mockResolvedValue(models),
    modelSuggest: vi.fn().mockResolvedValue({ pure_read: true, source: { rule_engine: true } }),
    switchSessionModel: opts.switchModel ?? vi.fn().mockResolvedValue({ model: "ollama" }),
  } as unknown as TenonApi;
}

const t = (k: string) => k;

describe("ModelRoutingPanel 对话框选择器", () => {
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

  it("按钮显示当前模型，点击弹出对话框并高亮当前项", async () => {
    render(<ModelRoutingPanel api={makeApi()} sessionId="s1" t={t} />);
    const trigger = await waitFor(() => {
      const el = screen.getByTestId("model-trigger");
      expect(el.textContent).toContain("glm");
      return el;
    });
    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeTruthy();
    const active = screen.getByTestId("model-option-glm");
    expect(active.getAttribute("class")).toContain("active");
    expect(screen.getByTestId("model-option-ollama").textContent).toContain(
      "model.local_free"
    );
  });

  it("点击模型经会话级切换 API 并关闭对话框", async () => {
    const switchModel = vi.fn().mockResolvedValue({ model: "ollama" });
    const onSwitched = vi.fn();
    render(<ModelRoutingPanel api={makeApi({ switchModel })} sessionId="s1" t={t} onSwitched={onSwitched} />);
    fireEvent.click(await screen.findByTestId("model-trigger"));
    fireEvent.click(screen.getByTestId("model-option-ollama"));
    await waitFor(() => expect(onSwitched).toHaveBeenCalledWith("ollama"));
    expect(switchModel).toHaveBeenCalledWith("s1", "ollama");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("无会话时选项禁用并提示走设置；Esc 关闭对话框", async () => {
    render(<ModelRoutingPanel api={makeApi()} sessionId={null} t={t} />);
    fireEvent.click(screen.getByTestId("model-trigger"));
    const option = await screen.findByTestId("model-option-glm");
    expect(option.hasAttribute("disabled")).toBe(true);
    expect(screen.getByRole("dialog").textContent).toContain("model.no_session_hint");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("点击遮罩关闭对话框", async () => {
    render(<ModelRoutingPanel api={makeApi()} sessionId="s1" t={t} />);
    fireEvent.click(screen.getByTestId("model-trigger"));
    fireEvent.click(screen.getByTestId("model-dialog-overlay"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("路由建议在对话框内提交并展示结果", async () => {
    render(<ModelRoutingPanel api={makeApi()} sessionId="s1" t={t} />);
    fireEvent.click(screen.getByTestId("model-trigger"));
    fireEvent.change(screen.getByTestId("suggest-input"), { target: { value: "读一下 README" } });
    fireEvent.click(screen.getByTestId("suggest-btn"));
    const result = await waitFor(() => screen.getByTestId("suggest-result"));
    expect(result.textContent).toContain("model.suggest_read");
  });
});
