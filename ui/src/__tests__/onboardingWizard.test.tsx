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
    putSecret: vi.fn().mockResolvedValue({ ok: true }),
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

  it("完成：先写钥匙串再写设置；putSettings 只含 api_key_env 引用名，永不落明文", async () => {
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
    // 顺序不变式：钥匙串写入成功后才提交设置
    expect(api.putSecret).toHaveBeenCalledWith("ZHIPU_API_KEY", "sk-secret-xyz");
    expect(
      (api.putSecret as ReturnType<typeof vi.fn>).mock.invocationCallOrder[0]
    ).toBeLessThan(
      (api.putSettings as ReturnType<typeof vi.fn>).mock.invocationCallOrder[0]
    );
    // 密钥不变式：settings 载荷只有引用名，明文只出现在 verify / secrets 调用里
    const payload = (api.putSettings as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(payload.models.default).toBe("glm");
    expect(payload.models.providers.glm).toEqual({
      kind: "openai",
      base_url: "https://open.bigmodel.cn/api/paas/v4",
      model: "glm-4.7-flash",
      api_key_env: "ZHIPU_API_KEY",
    });
    expect(JSON.stringify(payload)).not.toContain("sk-secret-xyz");
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
