// 语言包安装向导（设计方案 §8.4）：项目感知推荐 + 运行时检测 + 一键直执。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";

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
  onInstalled?: (language: string) => void;
}

export function LanguagePackWizard({ api, projectId, onInstalled }: Props) {
  const [packs, setPacks] = useState<PackInfo[]>([]);
  const [phase, setPhase] = useState<Record<string, string>>({});

  useEffect(() => {
    if (!projectId) return;
    api.detectLanguagePacks(projectId).then((r) => setPacks(r.packs ?? [])).catch(() => {});
  }, [api, projectId]);

  async function install(language: string) {
    const result = await api.installLanguagePack(projectId!, language);
    if (result.installed) {
      setPhase((p) => ({ ...p, [language]: "installed" }));
      onInstalled?.(language);
    }
  }

  const needing = packs.filter((p) => !p.server_installed);
  if (packs.length === 0) return null;

  return (
    <div className="lp-wizard" data-testid="language-pack-wizard">
      <div className="lp-title">语言包（项目感知推荐）</div>
      <ul>
        {packs.map((p) => (
          <li key={p.language} data-testid={`lp-${p.language}`}>
            <span>{p.language}</span>
            {p.server_installed ? (
              <span className="lp-ok">已就绪</span>
            ) : phase[p.language] === "installed" ? (
              <span className="lp-ok">刚安装 ✓</span>
            ) : (
              <span className="lp-missing">
                <span className="muted">{p.runtime_hint}</span>
                <button
                  data-testid={`lp-install-${p.language}`}
                  onClick={() => install(p.language)}
                >
                  一键安装
                </button>
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
