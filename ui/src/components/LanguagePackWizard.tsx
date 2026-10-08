// 语言包安装向导（设计方案 §8.4）：项目感知推荐 + 运行时检测 + 一键直执。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface PackInfo {
  language: string;
  command: string;
  detected: boolean;
  server_installed: boolean;
  runtime_hint: string | null;
}

interface Props {
  api: TenonApi;
  projectId: string | null;
  t: Translate;
  onInstalled?: (language: string) => void;
}

export function LanguagePackWizard({ api, projectId, t, onInstalled }: Props) {
  const [packs, setPacks] = useState<PackInfo[]>([]);
  const [phase, setPhase] = useState<Record<string, string>>({});
  const [installError, setInstallError] = useState<Record<string, string>>({});
  // 检测失败（v1.167）：不再静默为「无推荐」——向导区 inline 呈现错误
  const [detectError, setDetectError] = useState<string | null>(null);

  useEffect(() => {
    if (!projectId) return;
    api.detectLanguagePacks(projectId).then((r) => {
      setPacks(r.packs ?? []);
      setDetectError(null);
    }).catch((e) => {
      setPacks([]);
      setDetectError(String(e));
    });
  }, [api, projectId]);

  async function install(language: string) {
    try {
      const result = await api.installLanguagePack(projectId!, language);
      if (result.installed) {
        setPhase((p) => ({ ...p, [language]: "installed" }));
        onInstalled?.(language);
      }
    } catch (e) {
      // 失败必须可见：未捕获 rejection 会让点击像「没反应」
      setInstallError((p) => ({ ...p, [language]: String(e) }));
    }
  }

  const needing = packs.filter((p) => !p.server_installed);
  if (packs.length === 0) {
    if (!detectError) return null;
    return (
      <div className="lp-wizard" data-testid="language-pack-wizard">
        <div className="lp-title">{t("lp.title")}</div>
        <div className="tree-error" role="alert" data-testid="lp-detect-error">
          {detectError}
        </div>
      </div>
    );
  }

  return (
    <div className="lp-wizard" data-testid="language-pack-wizard">
      <div className="lp-title">{t("lp.title")}</div>
      <ul>
        {packs.map((p) => (
          <li key={p.language} data-testid={`lp-${p.language}`}>
            <span>{p.language}</span>
            {p.server_installed ? (
              <span className="lp-ok" role="status">
                {t("lp.ready")}
              </span>
            ) : phase[p.language] === "installed" ? (
              <span className="lp-ok" role="status" data-testid={`lp-installed-${p.language}`}>
                {t("lp.just_installed")}
              </span>
            ) : (
              <span className="lp-missing">
                <span className="muted">{p.runtime_hint}</span>
                {installError[p.language] && (
                  <span className="muted">{installError[p.language]}</span>
                )}
                <button
                  data-testid={`lp-install-${p.language}`}
                  onClick={() => install(p.language)}
                >
                  {t("lp.install")}
                </button>
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
