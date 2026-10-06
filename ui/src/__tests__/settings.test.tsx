// 设置面板（§7.2 / §15）：渲染回填 + 保存载荷 + 校验错误展示 + 模型分区（v1.40）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { SettingsDialog, type SettingsData } from "../components/SettingsDialog";
import type { TenonApi } from "../lib/api";

vi.mock("@monaco-editor/react", () => ({ default: () => null }));

const settings: SettingsData = {
  session: { mode: "interactive", first_edit_buffer_ms: 2000 },
  exec: { command_timeout_s: 120 },
};

function makeApi(
  put: ReturnType<typeof vi.fn> = vi.fn(),
  models: Record<string, unknown> = { models: [], default: "", laya: null }
) {
  return {
    putSettings: put,
    putTeamPolicy: vi.fn().mockResolvedValue({}),
    listPlugins: vi.fn().mockResolvedValue({ installed: [] }),
    models: vi.fn().mockResolvedValue(models),
  } as unknown as TenonApi;
}

const t = (k: string) => k;

/** v1.121：设置按分类渲染，组件测试先切换到对应导航页。 */
function openSection(name: "general" | "models" | "permissions" | "plugins" | "skills") {
  fireEvent.click(screen.getByTestId(`settings-nav-${name}`));
}

describe("SettingsDialog", () => {
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

  it("v1.121 分类导航：默认 General，切换后只渲染当前分类", () => {
    render(
      <SettingsDialog api={makeApi()} t={t} settings={settings} saveMode="auto" onSaveModeChange={() => {}} onClose={() => {}} onSaved={() => {}} />
    );
    expect(screen.getByTestId("settings-panel-general")).toBeTruthy();
    expect(screen.getByTestId("settings-save-mode")).toBeTruthy();

    openSection("models");
    expect(screen.queryByTestId("settings-panel-general")).toBeNull();
    expect(screen.getByTestId("settings-panel-models")).toBeTruthy();
    expect(screen.getByTestId("provider-preset")).toBeTruthy();
  });

  it("从全局设置回填表单", () => {
    render(
      <SettingsDialog api={makeApi()} t={t} settings={settings} saveMode="auto" onSaveModeChange={() => {}} onClose={() => {}} onSaved={() => {}} />
    );
    const buffer = screen.getByLabelText("settings.buffer") as HTMLInputElement;
    expect(buffer.value).toBe("2000");
    expect(screen.queryByLabelText("settings.mode")).toBeNull();
  });

  it("保存：载荷含会话与代理参数，成功后回调并关闭", async () => {
    const put = vi.fn().mockResolvedValue(settings);
    const onSaved = vi.fn();
    const onClose = vi.fn();
    render(
      <SettingsDialog api={makeApi(put)} t={t} settings={settings} saveMode="auto" onSaveModeChange={() => {}} onClose={onClose} onSaved={onSaved} />
    );
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(put).toHaveBeenCalledWith(
      expect.objectContaining({
        session: expect.objectContaining({ first_edit_buffer_ms: 2000 }),
        exec: expect.objectContaining({ command_timeout_s: 120 }),
      })
    );
    expect(onClose).toHaveBeenCalled();
  });

  it("保存失败：错误展示且不关闭", async () => {
    const put = vi.fn().mockRejectedValue(new Error("400 非法值"));
    const onClose = vi.fn();
    render(
      <SettingsDialog api={makeApi(put)} t={t} settings={settings} saveMode="auto" onSaveModeChange={() => {}} onClose={onClose} onSaved={() => {}} />
    );
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(onClose).not.toHaveBeenCalled();
  });

  it("模型分区：回填 provider 清单与 Laya 状态", async () => {
    const withModels: SettingsData = {
      ...settings,
      models: {
        default: "glm",
        providers: {
          glm: {
            kind: "openai",
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            model: "glm-4.7",
            overridden: false,
          },
        },
      },
    };
    render(
      <SettingsDialog
        api={makeApi(vi.fn(), {
          models: [],
          default: "glm",
          laya: { enabled: true, downloaded: true, version: "v2", device: "cpu" },
        })}
        t={t}
        settings={withModels}
        saveMode="auto"
        onSaveModeChange={() => {}}
        onClose={() => {}}
        onSaved={() => {}}
      />
    );
    openSection("models");
    const def = screen.getByTestId("settings-default-model") as HTMLSelectElement;
    expect(def.value).toBe("glm");
    expect(screen.getByTestId("provider-row-glm")).toBeTruthy();
    // 配置文件来源（未覆盖）行不提供删除
    expect(screen.queryByTestId("provider-remove-glm")).toBeNull();
    // Laya 状态卡只读展示
    const laya = await waitFor(() => screen.getByTestId("laya-status"));
    expect(laya.textContent).toContain("v2");
    expect(laya.textContent).toBeTruthy();
  });

  it("模型分区：预设添加 provider，保存发送 models 载荷且无明文密钥", async () => {
    const put = vi.fn().mockResolvedValue(settings);
    render(
      <SettingsDialog api={makeApi(put)} t={t} settings={settings} saveMode="auto" onSaveModeChange={() => {}} onClose={() => {}} onSaved={() => {}} />
    );
    openSection("models");
    fireEvent.change(screen.getByTestId("provider-preset"), { target: { value: "deepseek" } });
    fireEvent.click(screen.getByTestId("provider-add"));
    expect(screen.getByTestId("provider-row-deepseek")).toBeTruthy();
    const keyEnv = screen.getByLabelText(
      "settings.models.api_key_env · deepseek"
    ) as HTMLInputElement;
    fireEvent.change(keyEnv, { target: { value: "DEEPSEEK_API_KEY" } });
    fireEvent.change(screen.getByTestId("settings-default-model"), {
      target: { value: "deepseek" },
    });
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(put).toHaveBeenCalled());
    const payload = put.mock.calls[0][0];
    expect(payload.models).toEqual({
      default: "deepseek",
      providers: {
        deepseek: {
          kind: "openai",
          base_url: "https://api.deepseek.com",
          api_key_env: "DEEPSEEK_API_KEY",
        },
      },
    });
    // 密钥不变式：载荷只有 api_key_env 引用，无明文 api_key
    expect(JSON.stringify(payload)).not.toContain('"api_key"');
  });

  it("模型分区：删除覆盖 provider 后保存不再包含", async () => {
    const put = vi.fn().mockResolvedValue(settings);
    const withModels: SettingsData = {
      ...settings,
      models: {
        default: "",
        providers: {
          temp: { kind: "openai", base_url: "https://a.b", overridden: true },
        },
      },
    };
    render(
      <SettingsDialog api={makeApi(put)} t={t} settings={withModels} saveMode="auto" onSaveModeChange={() => {}} onClose={() => {}} onSaved={() => {}} />
    );
    openSection("models");
    fireEvent.click(screen.getByTestId("provider-remove-temp"));
    expect(screen.queryByTestId("provider-row-temp")).toBeNull();
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(put).toHaveBeenCalled());
    expect(put.mock.calls[0][0].models.providers).toEqual({});
  });

  it("v1.154 更新分区移除：导航无 updates 项，保存载荷不带 update 键", async () => {
    const withChannel = {
      ...settings,
      update: { channel: "auto" },
    } as SettingsData;
    const put = vi.fn().mockResolvedValue(withChannel);
    render(
      <SettingsDialog api={makeApi(put)} t={t} settings={withChannel} saveMode="auto" onSaveModeChange={() => {}} onClose={() => {}} onSaved={() => {}} />
    );
    // 存量 settings.json 的 update.channel 被忽略：无导航项、无面板、无控件
    expect(screen.queryByTestId("settings-nav-updates")).toBeNull();
    expect(screen.queryByTestId("settings-update-channel")).toBeNull();
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(put).toHaveBeenCalled());
    expect(put.mock.calls[0][0].update).toBeUndefined();
  });

  it("权限策略分区：回填收窄配置并原子保存", async () => {
    const withPolicy: SettingsData = {
      ...settings,
      team_policy: {
        force_interactive: true,
        denied_tools: ["git_push", "create_pr"],
        max_cost_usd: 1.5,
      },
    };
    const put = vi.fn().mockResolvedValue(withPolicy);
    const putPolicy = vi.fn().mockResolvedValue(withPolicy.team_policy);
    const api = {
      putSettings: put,
      putTeamPolicy: putPolicy,
      listPlugins: vi.fn().mockResolvedValue({ installed: [] }),
      models: vi.fn().mockResolvedValue({ models: [], default: "", laya: null }),
    } as unknown as TenonApi;
    render(
      <SettingsDialog api={api} t={t} settings={withPolicy} saveMode="auto" onSaveModeChange={() => {}} onClose={() => {}} onSaved={() => {}} />
    );
    openSection("permissions");
    expect(screen.queryByTestId("settings-force-interactive")).toBeNull();
    expect((screen.getByTestId("settings-denied-tools") as HTMLInputElement).value).toBe(
      "git_push, create_pr"
    );
    expect((screen.getByTestId("settings-policy-cost") as HTMLInputElement).value).toBe("1.5");

    fireEvent.change(screen.getByTestId("settings-denied-tools"), {
      target: { value: " git_push , mcp:* , " },
    });
    fireEvent.change(screen.getByTestId("settings-policy-cost"), { target: { value: "2" } });
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(putPolicy).toHaveBeenCalled());
    expect(putPolicy).toHaveBeenCalledWith({
      denied_tools: ["git_push", "mcp:*"],
      max_cost_usd: 2,
    });
    await waitFor(() => expect(put).toHaveBeenCalled());
  });

  it("权限策略分区：非法成本不提交", async () => {
    const put = vi.fn().mockResolvedValue(settings);
    const putPolicy = vi.fn().mockResolvedValue({});
    const api = {
      putSettings: put,
      putTeamPolicy: putPolicy,
      listPlugins: vi.fn().mockResolvedValue({ installed: [] }),
      models: vi.fn().mockResolvedValue({ models: [], default: "", laya: null }),
    } as unknown as TenonApi;
    render(
      <SettingsDialog api={api} t={t} settings={settings} saveMode="auto" onSaveModeChange={() => {}} onClose={() => {}} onSaved={() => {}} />
    );
    openSection("permissions");
    fireEvent.change(screen.getByTestId("settings-policy-cost"), { target: { value: "-1" } });
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(putPolicy).not.toHaveBeenCalled();
    expect(put).not.toHaveBeenCalled();
  });
});
