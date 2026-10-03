// 主工作区（设计方案 §7.2 四区布局）：文件树 | 编辑器 | 代理会话 + 底部时间轴。
// 三区可折叠；快捷键 §7.4。
import { useCallback, useMemo, useRef, useState } from "react";
import { TenonApi } from "./lib/api";
import { createTranslator, LOCALE_CHANGE, type Locale } from "./lib/i18n";
import type { AgentStateName } from "./lib/stateColors";
import { useShortcuts } from "./hooks";
import { FileTree } from "./components/FileTree";
import { EditorPane, type EditorTab } from "./components/EditorPane";
import { AgentPanel } from "./components/AgentPanel";
import { CheckpointTimeline } from "./components/CheckpointTimeline";
import { DiffPanel } from "./components/DiffPanel";
import { ThreePaneMerge, type DirtyConflict } from "./components/ThreePaneMerge";
import { ModelRoutingPanel } from "./components/ModelRoutingPanel";
import { AgentTracePanel } from "./components/AgentTracePanel";
import { EvalsPanel } from "./components/EvalsPanel";
import { LanguagePackWizard } from "./components/LanguagePackWizard";
import { unionLines } from "./lib/aiLines";
import { CommandPalette, type Command } from "./components/CommandPalette";

export default function App({
  handshake,
  projectPath,
  locale = "auto",
}: {
  handshake: { port: number; token: string };
  projectPath: string;
  locale?: Locale;
}) {
  const t = useMemo(() => createTranslator(locale), [locale]);
  const api = useMemo(() => new TenonApi(handshake), [handshake]);

  const [projectId, setProjectId] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [tabs, setTabs] = useState<EditorTab[]>([]);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [timelineOpen, setTimelineOpen] = useState(true);
  const [latestDiff, setLatestDiff] = useState<string | null>(null);
  const [dirtyConflict, setDirtyConflict] = useState<DirtyConflict | null>(null);
  const [aiLines, setAiLines] = useState<Record<string, number[]>>({});
  const projectIdRef = useRef<string | null>(null);
  const dirtyTimers = useRef<Map<string, number>>(new Map());
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [agentState, setAgentState] = useState<AgentStateName>("idle");
  const [routeNote, setRouteNote] = useState<string | null>(null);
  const [bottomTab, setBottomTab] = useState<"timeline" | "trace" | "evals">("timeline");

  // 打开项目 + 建会话（§7.3 打开项目流：TOFU 信任卡随 M1 全量；M0 默认交互档）
  const openProject = useCallback(async () => {
    const project = await api.registerProject(projectPath);
    await api.setTrust(project.id, true);
    setProjectId(project.id);
    projectIdRef.current = project.id;
    const session = await api.createSession(projectPath, "interactive");
    setSessionId(session.session_id);
  }, [api, projectPath]);

  const openFile = useCallback(
    async (path: string) => {
      if (tabs.some((tab) => tab.path === path)) {
        setActivePath(path);
        return;
      }
      const r = await api.readFile(path);
      setTabs((prev) => [...prev, { path, content: r.content }]);
      setActivePath(path);
    },
    [api, tabs]
  );

  const onStateChange = useCallback((s: AgentStateName) => setAgentState(s), []);
  const handlers = useMemo(
    () => ({
      onPalette: () => setPaletteOpen((v) => !v),
      onGotoFile: () => setPaletteOpen(true),
      onTimeline: () => setTimelineOpen((v) => !v),
      onSidebar: () => setSidebarOpen((v) => !v),
      onStop: () => sessionId && api.control(sessionId, "stop"),
      onPauseOrClose: () => {
        if (paletteOpen) setPaletteOpen(false);
        else if (sessionId) api.control(sessionId, "pause");
      },
    }),
    [api, sessionId, paletteOpen]
  );
  useShortcuts(handlers);

  const commands: Command[] = useMemo(
    () => [
      { id: "open.timeline", label: t("panel.timeline"), run: () => setTimelineOpen(true) },
      { id: "toggle.sidebar", label: t("panel.files"), run: () => setSidebarOpen((v) => !v) },
      {
        id: "agent.pause",
        label: t("message.pause"),
        run: () => sessionId && api.control(sessionId, "pause"),
      },
      {
        id: "agent.stop",
        label: t("message.stop"),
        run: () => sessionId && api.control(sessionId, "stop"),
      },
      {
        id: "agent.rollback",
        label: t("timeline.rollback"),
        run: () => sessionId && api.control(sessionId, "rollback"),
      },
      {
        id: "agent.unrollback",
        label: t("timeline.unrevert"),
        run: () => sessionId && api.control(sessionId, "unrollback"),
      },
    ],
    [t, api, sessionId]
  );

  void openProject; // 启动页（M0：挂载即开；M1 换项目选择页 + TOFU 卡）

  return (
    <div className="app" data-testid="app">
      <header className="app-head">
        <strong>{t("app.title")}</strong>
        <span className="muted">{t("app.subtitle")}</span>
        <span className="spacer" />
        <ModelRoutingPanel
          api={api}
          sessionId={sessionId}
          onSwitched={(m) => {
            // 切换提示（§11：上下文随迁，model_fallback 事件入 Trace）
            setRouteNote(`已切换模型：${m}（上下文随迁）`);
            window.setTimeout(() => setRouteNote(null), 4000);
          }}
        />
        <LanguagePicker />
      </header>
      {routeNote && (
        <div className="route-note" data-testid="route-note">
          {routeNote}
        </div>
      )}
      <div className="workspace">
        {sidebarOpen && (
          <aside className="zone zone-left">
            <LanguagePackWizard
              api={api}
              projectId={projectId}
              onInstalled={() => {
                // 重新拉取文件树无必要；向导自身刷新状态
              }}
            />
            <FileTree api={api} t={t} projectId={projectId} onOpenFile={openFile} />
          </aside>
        )}
        <section className="zone zone-center">
          <EditorPane
            tabs={tabs}
            activePath={activePath}
            aiModifiedLines={aiLines}
            onSelect={setActivePath}
            onClose={(p) => {
              setTabs((prev) => prev.filter((tab) => tab.path !== p));
              setActivePath((prev) =>
                prev === p ? (tabs.find((tab) => tab.path !== p)?.path ?? null) : prev
              );
            }}
            onChange={(p, content) => {
              setTabs((prev) => prev.map((tab) => (tab.path === p ? { ...tab, content } : tab)));
              // §8.6：用户编辑 → 该文件 AI 角标解除 + 脏缓冲推送（去抖）
              setAiLines((prev) => ({ ...prev, [p]: [] }));
              const pid = projectIdRef.current;
              if (pid) {
                const tid = dirtyTimers.current.get(p);
                if (tid) window.clearTimeout(tid);
                dirtyTimers.current.set(
                  p,
                  window.setTimeout(() => {
                    void api.putBuffer(pid, p, content).catch(() => {});
                  }, 400)
                );
              }
            }}
          />
        </section>
        <section className="zone zone-right">
          <AgentPanel
            api={api}
            t={t}
            sessionId={sessionId}
            onStateChange={onStateChange}
            onLatestDiff={setLatestDiff}
            onPatchLines={(path, lines) =>
              setAiLines((prev) => ({
                ...prev,
                [path]: unionLines(prev[path] ?? [], lines),
              }))
            }
            onDirtyConflict={setDirtyConflict}
          />
        </section>
      </div>
      {dirtyConflict && (
        <div className="merge-overlay">
          <ThreePaneMerge
            conflict={dirtyConflict}
            labels={{
              ours: "代理改动",
              theirs: "你的改动（未保存）",
              base: "自动合并结果",
              apply: "应用选中版本并保存",
              keepMine: "保留我的版本",
              useAgent: "应用代理版本",
            }}
            onResolve={(path, chosen, clear) => {
              void (async () => {
                await api.writeFile(path, chosen).catch(() => {});
                const pid = projectIdRef.current;
                if (pid && clear) await api.clearBuffer(pid, path).catch(() => {});
                setTabs((prev) =>
                  prev.some((tab) => tab.path === path)
                    ? prev.map((tab) => (tab.path === path ? { ...tab, content: chosen } : tab))
                    : [...prev, { path, content: chosen }]
                );
                setActivePath(path);
                setDirtyConflict(null);
              })();
            }}
          />
        </div>
      )}
      {timelineOpen && (
        <footer className="zone-bottom">
          <div className="bottom-tabs">
            <button
              className={bottomTab === "timeline" ? "active" : ""}
              onClick={() => setBottomTab("timeline")}
            >
              {t("panel.timeline")}
            </button>
            <button
              className={bottomTab === "trace" ? "active" : ""}
              onClick={() => setBottomTab("trace")}
              data-testid="tab-trace"
            >
              AgentTrace
            </button>
            <button
              className={bottomTab === "evals" ? "active" : ""}
              onClick={() => setBottomTab("evals")}
              data-testid="tab-evals"
            >
              AI Evals
            </button>
          </div>
          {bottomTab === "timeline" && (
            <div className="bottom-grid">
              <CheckpointTimeline api={api} t={t} sessionId={sessionId} />
              <DiffPanel diff={latestDiff} title={t("panel.diagnostics")} />
            </div>
          )}
          {bottomTab === "trace" && (
            <AgentTracePanel api={api} sessionId={sessionId} />
          )}
          {bottomTab === "evals" && <EvalsPanel api={api} />}
        </footer>
      )}
      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} commands={commands} />
      <span className="sr-only" data-testid="agent-state">{agentState}</span>
    </div>
  );
}

function LanguagePicker() {
  const [locale, setLocaleState] = useState<Locale>("auto");
  return (
    <select
      aria-label="language"
      value={locale}
      onChange={(e) => {
        const v = e.target.value as Locale;
        setLocaleState(v);
        window.dispatchEvent(new CustomEvent(LOCALE_CHANGE, { detail: v }));
      }}
    >
      <option value="auto">Auto</option>
      <option value="en">English</option>
      <option value="zh-CN">中文</option>
    </select>
  );
}
