import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { TenonApi, readPairedToken, savePairedToken } from "./lib/api";
import {
  createTranslator,
  isLocale,
  loadLocaleResource,
  readLocalePreference,
  resolveLocale,
  saveLocalePreference,
  type Translate,
} from "./lib/i18n";
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
 *  偏好源与 App 一致——localStorage `tenon:locale`（ui-prefs 为跨启动权威，boot 内对齐）。 */
function bootTranslator(): Translate {
  return createTranslator(readLocalePreference());
}

let t = bootTranslator();

/** 品牌启动屏：握手轮询期间即渲染（桌面端最多等 10s，不再白屏）。 */
function renderSplash(): void {
  const el = document.getElementById("root");
  if (!el) return;
  el.innerHTML = `
    <div id="boot-splash">
      <div class="boot-mark">T</div>
      <div class="boot-name">Tenon Harness</div>
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

/** 同源自发现：仅当页面本身由 daemon 托管（浏览器访问）时可达。
 *  局域网已配对设备携带设备令牌（v1.157 §12.6：Host 非回环时中间件要求）。 */
async function fromSameOrigin(): Promise<Handshake | null> {
  try {
    const paired = readPairedToken();
    const headers: Record<string, string> = {};
    if (paired) headers["X-Tenon-Paired"] = paired;
    const resp = await fetch("/pairing", { cache: "no-store", headers });
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

/** 非回环主机 = 经 IP 直访的局域网设备（v1.157）：握手被拒时进入配对屏。 */
function isLanHost(): boolean {
  const h = location.hostname;
  return h !== "127.0.0.1" && h !== "localhost" && h !== "::1" && h !== "[::1]";
}

/** 局域网配对屏（v1.157 §12.6）：设备名 + 6 位一次性码换设备令牌（唯一免令牌
 *  端点 /lan/pair）；成功落 localStorage 后重跑同源握手。取消/关闭即停留。 */
function lanPairFlow(): Promise<Handshake | null> {
  return new Promise((resolve) => {
    const el = document.getElementById("root");
    if (!el) return resolve(null);
    el.innerHTML = `
      <div id="boot-splash" class="boot-pair">
        <div class="boot-mark">T</div>
        <div class="boot-name">${escapeHtml(t("lan.pair_title"))}</div>
        <p class="boot-note">${escapeHtml(t("lan.pair_hint"))}</p>
        <form id="lan-pair-form">
          <input id="lan-pair-device" maxlength="60" data-testid="lan-pair-device"
            placeholder="${escapeHtml(t("lan.device"))}"
            value="${escapeHtml(t("lan.device_default"))}" />
          <input id="lan-pair-code" inputmode="numeric" maxlength="6" data-testid="lan-pair-code"
            placeholder="${escapeHtml(t("lan.code"))}" autocomplete="one-time-code" />
          <button type="submit" id="lan-pair-submit" data-testid="lan-pair-submit">
            ${escapeHtml(t("lan.pair"))}
          </button>
        </form>
        <p class="boot-note boot-error" id="lan-pair-error" hidden></p>
      </div>`;
    const form = document.getElementById("lan-pair-form") as HTMLFormElement | null;
    const deviceInput = document.getElementById("lan-pair-device") as HTMLInputElement | null;
    const codeInput = document.getElementById("lan-pair-code") as HTMLInputElement | null;
    const submitBtn = document.getElementById("lan-pair-submit") as HTMLButtonElement | null;
    const errorEl = document.getElementById("lan-pair-error");
    if (!form || !deviceInput || !codeInput || !submitBtn || !errorEl) return resolve(null);
    const fail = (message: string) => {
      errorEl.textContent = message;
      errorEl.hidden = false;
      submitBtn.disabled = false;
    };
    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      const code = codeInput.value.trim();
      const device = deviceInput.value.trim() || t("lan.device_default");
      if (!/^\d{6}$/.test(code)) return fail(t("lan.pair_invalid"));
      submitBtn.disabled = true;
      errorEl.hidden = true;
      fetch("/lan/pair", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ device, code }),
      })
        .then(async (resp) => {
          if (!resp.ok) return fail(t("lan.pair_failed"));
          const d = (await resp.json()) as { paired_token?: string };
          if (!d.paired_token) return fail(t("lan.pair_failed"));
          savePairedToken(d.paired_token);
          // 配对成功 → 重跑同源握手（携带设备令牌）；再次 403 视为失败
          resolve(await fromSameOrigin());
        })
        .catch(() => fail(t("lan.pair_failed")));
    });
  });
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
    // 语言（v1.100）：按本地偏好懒加载对应语言包后再渲染启动屏（分块仅几 KB）；
    // 握手后以 ui-prefs 为权威对齐，下一行渲染的 App 经 localStorage 读到最终偏好
    await loadLocaleResource(resolveLocale(readLocalePreference()));
    t = bootTranslator();
    renderSplash();
    let handshake: Handshake;
    try {
      handshake = await discoverHandshake();
    } catch (e) {
      // 局域网未配对设备（v1.157 §12.6）：握手被中间件 403 拦截 → 配对屏，
      // 配对成功返回握手继续引导；失败 / 无根节点回落原错误屏
      const paired = isLanHost() ? await lanPairFlow() : null;
      if (!paired) throw e;
      handshake = paired;
    }
    const prefs = await new TenonApi(handshake).getUiPrefs();
    if (isThemePreference(prefs.theme) && prefs.theme !== loadThemePreference()) {
      saveThemePreference(prefs.theme);
    }
    if (isLocale(prefs.locale) && prefs.locale !== readLocalePreference()) {
      saveLocalePreference(prefs.locale);
      await loadLocaleResource(resolveLocale(prefs.locale));
      t = bootTranslator();
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
