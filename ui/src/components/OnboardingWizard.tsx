// 接入免费模型向导（§7.1 / §11 / §15 v1.163，v1.165 简化）：智谱 GLM-4.7-Flash
// 官方免费档开箱引导——三步（介绍 → 注册拿 Key → 验证并一键写入配置）。首启
// 合并视图无 provider 且未跳过时自动弹出（App 接线）；模型空态与设置 Models 可重开。
// 密钥直存（v1.165 §11 双轨）：验证经 POST /models/verify，完成 = 单次
// PUT /settings 把 api_key 与 provider 一并写入 settings.json（0600）并热生效；
// GET /settings 永不回显密钥。
import { useState } from "react";
import type {
  SettingsData,
  TenonApi,
  VerifyModelResult,
} from "../lib/api";
import type { Translate } from "../lib/i18n";

/** 智谱免费档预设（§11 v1.163）：OpenAI 兼容端点 + 官方免费模型。 */
export const GLM_PRESET = {
  name: "glm",
  baseUrl: "https://open.bigmodel.cn/api/paas/v4",
  model: "glm-4.7-flash",
} as const;

const ZHIPU_PORTAL_URL = "https://open.bigmodel.cn";
const ZHIPU_APIKEYS_URL = "https://open.bigmodel.cn/usercenter/apikeys";
/** 跳过记忆键（ui-prefs，§7.5）：置 "1" 后不再自动弹出。 */
export const ONBOARDING_DISMISSED_KEY = "onboarding.dismissed";

type Step = "intro" | "key" | "done";

interface Props {
  api: TenonApi;
  t: Translate;
  /** 当前合并视图设置：完成时在其 provider 表上合并 glm 条目后整体提交。 */
  settings: SettingsData | null;
  onDone: (settings: SettingsData) => void;
  onSkip: () => void;
}

export function OnboardingWizard({ api, t, settings, onDone, onSkip }: Props) {
  const [step, setStep] = useState<Step>("intro");
  const [apiKey, setApiKey] = useState("");
  const [verifying, setVerifying] = useState(false);
  const [verified, setVerified] = useState<VerifyModelResult | null>(null);
  const [verifyError, setVerifyError] = useState<string | null>(null);
  const [finishing, setFinishing] = useState(false);
  const [finishError, setFinishError] = useState<string | null>(null);

  function skip() {
    // 跳过记忆持久化（fire-and-forget）：本会话内也不再自动弹
    api.setUiPrefs({ [ONBOARDING_DISMISSED_KEY]: "1" });
    onSkip();
  }

  async function verify() {
    if (!apiKey.trim() || verifying) return;
    setVerifying(true);
    setVerifyError(null);
    setVerified(null);
    try {
      const r = await api.verifyModel({
        kind: "openai",
        base_url: GLM_PRESET.baseUrl,
        model: GLM_PRESET.model,
        api_key: apiKey.trim(),
      });
      setVerified(r);
      setStep("done");
    } catch (e) {
      setVerifyError(String(e));
    } finally {
      setVerifying(false);
    }
  }

  async function finish() {
    if (finishing) return;
    setFinishing(true);
    setFinishError(null);
    try {
      // provider 覆盖表整体替换（§15）：合并既有条目避免误删用户配置；
      // api_key 直存 settings.json（0600，GET 不回显）
      const existing = settings?.models?.providers ?? {};
      const next = await api.putSettings({
        models: {
          default: GLM_PRESET.name,
          providers: {
            ...existing,
            [GLM_PRESET.name]: {
              kind: "openai",
              base_url: GLM_PRESET.baseUrl,
              model: GLM_PRESET.model,
              api_key: apiKey.trim(),
            },
          },
        },
      });
      onDone(next);
    } catch (e) {
      setFinishError(String(e));
    } finally {
      setFinishing(false);
    }
  }

  const stepIndex = step === "intro" ? 0 : step === "key" ? 1 : 2;

  return (
    <div className="merge-overlay" data-testid="onboarding-overlay">
      <div
        className="merge-pane onboarding-pane"
        role="dialog"
        aria-label={t("onboarding.title")}
        data-testid="onboarding-dialog"
      >
        <div className="merge-head onboarding-head">
          <strong>{t("onboarding.title")}</strong>
          <ol className="onboarding-steps" data-testid="onboarding-steps">
            {(["intro", "key", "done"] as Step[]).map((s, i) => (
              <li
                key={s}
                className={
                  i === stepIndex
                    ? "onboarding-step active"
                    : i < stepIndex
                      ? "onboarding-step done"
                      : "onboarding-step"
                }
                aria-current={i === stepIndex ? "step" : undefined}
              >
                {t(`onboarding.step_${s}`)}
              </li>
            ))}
          </ol>
        </div>

        {step === "intro" && (
          <div className="onboarding-body" data-testid="onboarding-step-intro">
            <p>{t("onboarding.intro")}</p>
            <div className="onboarding-model-card" data-testid="onboarding-model-card">
              <code>{GLM_PRESET.model}</code>
              <span className="onboarding-free-badge">{t("onboarding.free_badge")}</span>
            </div>
            <p className="muted settings-note">{t("onboarding.intro_note")}</p>
            <div className="onboarding-actions">
              <button type="button" onClick={skip} data-testid="onboarding-skip">
                {t("onboarding.skip")}
              </button>
              <span className="onboarding-actions-gap" />
              <a
                className="onboarding-portal-link"
                href={ZHIPU_PORTAL_URL}
                target="_blank"
                rel="noreferrer"
                data-testid="onboarding-open-portal"
              >
                {t("onboarding.open_portal")}
              </a>
              <button
                type="button"
                className="onboarding-primary"
                data-testid="onboarding-next"
                onClick={() => setStep("key")}
              >
                {t("onboarding.next")}
              </button>
            </div>
          </div>
        )}

        {step === "key" && (
          <div className="onboarding-body" data-testid="onboarding-step-key">
            <p>{t("onboarding.key_instruction")}</p>
            <div className="onboarding-key-row">
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder={t("onboarding.key_placeholder")}
                aria-label={t("onboarding.key_placeholder")}
                value={apiKey}
                onChange={(e) => {
                  setApiKey(e.target.value);
                  setVerified(null);
                }}
                data-testid="onboarding-key-input"
              />
              <a
                className="onboarding-portal-link"
                href={ZHIPU_APIKEYS_URL}
                target="_blank"
                rel="noreferrer"
                data-testid="onboarding-open-keys"
              >
                {t("onboarding.open_keys")}
              </a>
            </div>
            {verifyError && (
              <div className="onboarding-error" role="alert" data-testid="onboarding-verify-error">
                {t("onboarding.verify_failed")}
                <span className="muted"> {verifyError}</span>
              </div>
            )}
            <div className="onboarding-actions">
              <button
                type="button"
                disabled={verifying}
                onClick={() => setStep("intro")}
                data-testid="onboarding-back"
              >
                {t("onboarding.back")}
              </button>
              <span className="onboarding-actions-gap" />
              <button
                type="button"
                className="onboarding-primary"
                disabled={!apiKey.trim() || verifying}
                data-testid="onboarding-verify"
                onClick={() => void verify()}
              >
                {verifying ? t("onboarding.verifying") : t("onboarding.verify")}
              </button>
            </div>
          </div>
        )}

        {step === "done" && (
          <div className="onboarding-body" data-testid="onboarding-step-done">
            <p data-testid="onboarding-verify-ok">
              {t("onboarding.verify_ok", {
                model: verified?.model ?? GLM_PRESET.model,
                latency: verified?.latency_ms ?? 0,
              })}
            </p>
            <p className="muted">{t("onboarding.done_intro")}</p>
            {finishError && (
              <div className="onboarding-error" role="alert" data-testid="onboarding-finish-error">
                {t("onboarding.finish_failed")}
                <span className="muted"> {finishError}</span>
              </div>
            )}
            <div className="onboarding-actions">
              <button
                type="button"
                disabled={finishing}
                onClick={() => setStep("key")}
                data-testid="onboarding-back-from-done"
              >
                {t("onboarding.back")}
              </button>
              <span className="onboarding-actions-gap" />
              <button
                type="button"
                className="onboarding-primary"
                disabled={finishing}
                data-testid="onboarding-finish"
                onClick={() => void finish()}
              >
                {finishing ? t("onboarding.finishing") : t("onboarding.finish")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
