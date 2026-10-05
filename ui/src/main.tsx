import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { TenonApi } from "./lib/api";
import { createTranslator, type Locale, type Translate } from "./lib/i18n";
import { applyTheme, isThemePreference, loadThemePreference, saveThemePreference } from "./lib/theme";
import "./styles.css";

// 握手来源优先级：
// 1. URL 查询参数 ?port=&token=&project=（桌面壳导航 / 显式链接）
// 2. Tauri 桌面壳：window.__TAURI_INTERNALS__.invoke("get_handshake")（轮询直至就绪）
// 3. 同源自发现：fetch /pairing（daemon 同源托管 UI 时；回传 daemon 注册的项目根）
declare global {
  interface Window {
    __TENON_HANDSHAKE__?: { port: number; token: string; project?: string };
    __TENON_PROJECT__?: string;
    __TAURI_INTERNALS__?: Record<string, unknown>;
  }
}

interface Handshake {
  port: number;
  token: string;
  project?: string;
}

function fromParams(): Handshake | null {
  const params = new URLSearchParams(location.search);
  const port = params.get("port");
  const token = params.get("token");
  if (port && token)
    return {
      port: Number(port),
      token,
      project: params.get("project") ?? undefined,
    };
  return null;
}

/** Tauri IPC invoke（Tauri 2 自动注入 __TAURI_INTERNALS__）。 */
async function tauriInvoke<T>(cmd: string): Promise<T> {
  const internals = window.__TAURI_INTERNALS__ as
    | { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<T> }
    | undefined;
  if (!internals) throw new Error("not in Tauri");
  return internals.invoke(cmd);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** 桌面壳标记（§6.2 集成式标题栏：macOS 红绿灯内边距经 CSS 生效）。 */
if (window.__TAURI_INTERNALS__) {
  document.documentElement.classList.add("is-tauri");
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c] ?? c));
}

/** 启动屏文案随应用语言（§4.2）：React 挂载前直用纯函数翻译器，
 *  偏好源与 App 一致——localStorage `tenon:locale`，缺省 auto 跟随系统。 */
function bootTranslator(): Translate {
  let pref: Locale = "auto";
  try {
    const raw = localStorage.getItem("tenon:locale");
    if (raw === "en" || raw === "zh-CN" || raw === "auto") pref = raw;
  } catch {
    // 存储不可用：维持 auto
  }
  return createTranslator(pref);
}

const t = bootTranslator();

/** 品牌启动屏：握手轮询期间即渲染（桌面端最多等 10s，不再白屏）。 */
function renderSplash(): void {
  const el = document.getElementById("root");
  if (!el) return;
  el.innerHTML = `
    <div id="boot-splash">
      <div class="boot-mark">T</div>
      <div class="boot-name">Tenon</div>
      <div class="boot-note">${escapeHtml(t("boot.connecting"))}</div>
      <div class="boot-dots"><i></i><i></i><i></i></div>
    </div>`;
}

/** Tauri 桌面壳：daemon 在后台线程启动，握手可能晚于页面就绪——轮询等待。 */
async function fromTauri(maxMs = 10_000): Promise<Handshake | null> {
  if (!window.__TAURI_INTERNALS__) return null;
  const deadline = Date.now() + maxMs;
  while (Date.now() < deadline) {
    try {
      const hs = await tauriInvoke<Handshake>("get_handshake");
      if (hs.port && hs.token) return hs;
    } catch {
      // daemon 未就绪，继续等
    }
    await sleep(300);
  }
  return null;
}

/** 同源自发现：仅当页面本身由 daemon 托管（浏览器访问）时可达。 */
async function fromSameOrigin(): Promise<Handshake | null> {
  try {
    const resp = await fetch("/pairing", { cache: "no-store" });
    if (resp.ok) {
      const d = await resp.json();
      if (d.port && d.token)
        return { port: d.port, token: d.token, project: d.project ?? undefined };
    }
  } catch {
    // 不可达（Tauri asset 协议下必然失败）
  }
  return null;
}

async function discoverHandshake(): Promise<Handshake> {
  if (window.__TENON_HANDSHAKE__) return window.__TENON_HANDSHAKE__;

  const fromQuery = fromParams();
  if (fromQuery) return fromQuery;

  const viaTauri = await fromTauri();
  if (viaTauri) return viaTauri;

  const viaOrigin = await fromSameOrigin();
  if (viaOrigin) return viaOrigin;

  if (import.meta.env.DEV) return { port: 9876, token: "dev" };
  throw new Error(t("boot.connect_failed"));
}

function projectPath(handshake: Handshake): string {
  return (
    new URLSearchParams(location.search).get("project") ??
    handshake.project ??
    window.__TENON_PROJECT__ ??
    "."
  );
}

function renderFatal(message: string) {
  const el = document.getElementById("root");
  if (!el) return;
  el.innerHTML = `
    <div id="boot-splash" class="boot-failed">
      <div class="boot-mark">T</div>
      <div class="boot-name">${escapeHtml(t("boot.failed_title"))}</div>
      <p class="boot-note boot-error">${escapeHtml(message)}</p>
      <div class="boot-hint">${escapeHtml(t("boot.hint"))}</div>
    </div>`;
}

async function boot() {
  const rootEl = document.getElementById("root");
  if (!rootEl) {
    renderFatal(t("boot.root_missing"));
    return;
  }
  try {
    // 外观（§7.5）：先按本地缓存应用避免闪烁；daemon 端口动态导致
    // localStorage 按 origin 隔离，跨启动以 /ui-prefs 为权威再对齐
    applyTheme(loadThemePreference());
    renderSplash();
    const handshake = await discoverHandshake();
    const prefs = await new TenonApi(handshake).getUiPrefs();
    if (isThemePreference(prefs.theme) && prefs.theme !== loadThemePreference()) {
      saveThemePreference(prefs.theme);
    }
    applyTheme(loadThemePreference());
    rootEl.innerHTML = "";
    const root = createRoot(rootEl);
    root.render(
      <StrictMode>
        <App handshake={handshake} projectPath={projectPath(handshake)} />
      </StrictMode>
    );
  } catch (e) {
    renderFatal(e instanceof Error ? e.message : String(e));
  }
}

boot();
