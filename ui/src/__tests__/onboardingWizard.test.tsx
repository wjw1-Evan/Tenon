// 接入免费模型向导（§7.1 / §15 v1.163）：三步流转 + 试连载荷 + 完成写入
// （钥匙串先于 settings、密钥明文不进 putSettings 载荷）+ 跳过记忆 + 入口 CTA。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { OnboardingWizard } from "../components/OnboardingWizard";
import { SettingsDialog } from "../components/SettingsDialog";
import type { SettingsData, TenonApi } from "../lib/api";

const NEXT_SETTINGS: SettingsData = {
  session: {},
  models: { default: "glm", providers: {} },
};

function makeApi(overrides: Record<string, ReturnType<typeof vi.fn>> = {}) {
  return {
    verifyModel: vi
      .fn()
      .mockResolvedValue({ ok: true, model: "glm-4.7-flash", latency_ms: 42 }),
    putSettings: vi.fn().mockResolvedValue(NEXT_SETTINGS),
    setUiPrefs: vi.fn(),
    models: vi.fn().mockResolvedValue({ models: [], default: "", laya: null }),
    ...overrides,
  } as unknown as TenonApi;
}

const t = (k: string) => k;

async function walkToDone(api: TenonApi, key = "sk-test-abc") {
  render(
    <OnboardingWizard api={api} t={t} settings={{ session: {} }} onDone={() => {}} onSkip={() => {}} />
  );
  fireEvent.click(screen.getByTestId("onboarding-next"));
  fireEvent.change(screen.getByTestId("onboarding-key-input"), {
    target: { value: key },
  });
  fireEvent.click(screen.getByTestId("onboarding-verify"));
  await screen.findByTestId("onboarding-step-done");
}

describe("OnboardingWizard", () => {
  it("三步流转：介绍 → 贴 Key 验证（载荷含预设端点与模型）→ 完成步", async () => {
    const api = makeApi();
    await walkToDone(api);
    expect(api.verifyModel).toHaveBeenCalledWith({
      kind: "openai",
      base_url: "https://open.bigmodel.cn/api/paas/v4",
      model: "glm-4.7-flash",
      api_key: "sk-test-abc",
    });
    expect(screen.getByTestId("onboarding-verify-ok")).toBeTruthy();
  });

  it("验证失败：错误行可见且停留在贴 Key 步", async () => {
    const api = makeApi({
      verifyModel: vi.fn().mockRejectedValue(new Error("API 400: HTTP 401: invalid")),
    });
    render(
      <OnboardingWizard api={api} t={t} settings={{ session: {} }} onDone={() => {}} onSkip={() => {}} />
    );
    fireEvent.click(screen.getByTestId("onboarding-next"));
    fireEvent.change(screen.getByTestId("onboarding-key-input"), {
      target: { value: "sk-bad" },
    });
    fireEvent.click(screen.getByTestId("onboarding-verify"));
    await waitFor(() => screen.getByTestId("onboarding-verify-error"));
    expect(screen.queryByTestId("onboarding-step-done")).toBeNull();
    expect(screen.getByTestId("onboarding-step-key")).toBeTruthy();
  });

  it("v1.169 免费档自动回退：首选 429 限流自动改试 glm-4.5-flash 并按其写入", async () => {
    const api = makeApi({
      verifyModel: vi.fn((payload: { model: string }) =>
        payload.model === "glm-4.7-flash"
          ? Promise.reject(new Error("API 400: HTTP 429: 该模型当前访问量过大"))
          : Promise.resolve({ ok: true, model: "glm-4.5-flash", latency_ms: 30 })
      ),
    });
    const onDone = vi.fn();
    render(
      <OnboardingWizard api={api} t={t} settings={{ session: {} }} onDone={onDone} onSkip={() => {}} />
    );
    fireEvent.click(screen.getByTestId("onboarding-next"));
    fireEvent.change(screen.getByTestId("onboarding-key-input"), {
      target: { value: "sk-fallback" },
    });
    fireEvent.click(screen.getByTestId("onboarding-verify"));
    await screen.findByTestId("onboarding-step-done");
    // 回退标注可见
    expect(screen.getByTestId("onboarding-fallback-note")).toBeTruthy();
    // 完成按后备模型写入
    fireEvent.click(screen.getByTestId("onboarding-finish"));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    const payload = (api.putSettings as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(payload.models.providers.glm.model).toBe("glm-4.5-flash");
    // 两次调用：先首选后后备
    expect((api.verifyModel as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0].model)).toEqual([
      "glm-4.7-flash",
      "glm-4.5-flash",
    ]);
  });

  it("v1.169 fetch 层失败映射友好文案（不再裸 TypeError）", async () => {
    const api = makeApi({
      verifyModel: vi.fn().mockRejectedValue(new TypeError("Load failed")),
    });
    render(
      <OnboardingWizard api={api} t={t} settings={{ session: {} }} onDone={() => {}} onSkip={() => {}} />
    );
    fireEvent.click(screen.getByTestId("onboarding-next"));
    fireEvent.change(screen.getByTestId("onboarding-key-input"), {
      target: { value: "sk-net" },
    });
    fireEvent.click(screen.getByTestId("onboarding-verify"));
    await waitFor(() => screen.getByTestId("onboarding-verify-error"));
    expect(screen.getByTestId("onboarding-verify-error").textContent).toContain(
      "onboarding.daemon_unreachable"
    );
    expect(screen.getByTestId("onboarding-verify-error").textContent).not.toContain("TypeError");
    // 两个候选档都试过才报错
    expect(api.verifyModel).toHaveBeenCalledTimes(2);
  });

  it("完成：单次 putSettings 直存 api_key（v1.165 简化，不再经钥匙串）", async () => {
    const api = makeApi();
    const onDone = vi.fn();
    render(
      <OnboardingWizard api={api} t={t} settings={{ session: {} }} onDone={onDone} onSkip={() => {}} />
    );
    fireEvent.click(screen.getByTestId("onboarding-next"));
    fireEvent.change(screen.getByTestId("onboarding-key-input"), {
      target: { value: "sk-secret-xyz" },
    });
    fireEvent.click(screen.getByTestId("onboarding-verify"));
    await screen.findByTestId("onboarding-step-done");
    fireEvent.click(screen.getByTestId("onboarding-finish"));

    await waitFor(() => expect(onDone).toHaveBeenCalled());
    const payload = (api.putSettings as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(payload.models.default).toBe("glm");
    // 直存轨（§11 v1.165）：api_key 明文随 provider 一并写入 settings.json（0600）
    expect(payload.models.providers.glm).toEqual({
      kind: "openai",
      base_url: "https://open.bigmodel.cn/api/paas/v4",
      model: "glm-4.7-flash",
      api_key: "sk-secret-xyz",
    });
    expect(onDone).toHaveBeenCalledWith(NEXT_SETTINGS);
  });

  it("完成：合并既有 provider 覆盖表，不误删用户配置", async () => {
    const api = makeApi();
    const existing: SettingsData = {
      session: {},
      models: {
        providers: { foo: { kind: "anthropic", base_url: "https://x", overridden: true } },
      },
    };
    render(
      <OnboardingWizard api={api} t={t} settings={existing} onDone={() => {}} onSkip={() => {}} />
    );
    fireEvent.click(screen.getByTestId("onboarding-next"));
    fireEvent.change(screen.getByTestId("onboarding-key-input"), {
      target: { value: "sk-keep" },
    });
    fireEvent.click(screen.getByTestId("onboarding-verify"));
    await screen.findByTestId("onboarding-step-done");
    fireEvent.click(screen.getByTestId("onboarding-finish"));
    await waitFor(() =>
      expect(api.putSettings).toHaveBeenCalled()
    );
    const payload = (api.putSettings as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(payload.models.providers.foo).toEqual({
      kind: "anthropic",
      base_url: "https://x",
      overridden: true,
    });
    expect(payload.models.providers.glm).toBeTruthy();
  });

  it("跳过：ui-prefs 记忆 dismiss 键并回调 onSkip", () => {
    const api = makeApi();
    const onSkip = vi.fn();
    render(
      <OnboardingWizard api={api} t={t} settings={{ session: {} }} onDone={() => {}} onSkip={onSkip} />
    );
    fireEvent.click(screen.getByTestId("onboarding-skip"));
    expect(api.setUiPrefs).toHaveBeenCalledWith({ "onboarding.dismissed": "1" });
    expect(onSkip).toHaveBeenCalled();
  });

  it("入口 CTA：设置 Models 分区与模型空态渲染引导按钮并回调", () => {
    const settings: SettingsData = { session: {}, exec: {} };
    const onOpenOnboarding = vi.fn();
    render(
      <SettingsDialog
        api={makeApi()}
        t={t}
        settings={settings}
        saveMode="auto"
        onSaveModeChange={() => {}}
        onClose={() => {}}
        onSaved={() => {}}
        onOpenOnboarding={onOpenOnboarding}
      />
    );
    fireEvent.click(screen.getByTestId("settings-nav-models"));
    fireEvent.click(screen.getByTestId("settings-models-onboarding"));
    expect(onOpenOnboarding).toHaveBeenCalled();
  });
});
