import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
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
  throw new Error(
    "无法连接 Tenon 内核（daemon 未就绪）。请重启应用；若持续失败请查看日志。"
  );
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
    <div style="display:flex;flex-direction:column;gap:12px;align-items:center;justify-content:center;height:100vh;font-family:system-ui;color:#e5e7eb;background:#111827;padding:24px;text-align:center">
      <h1 style="font-size:18px;margin:0">Tenon 启动失败</h1>
      <p style="max-width:480px;line-height:1.6;color:#9ca3af;margin:0;font-size:13px">${message}</p>
    </div>`;
}

async function boot() {
  const rootEl = document.getElementById("root");
  if (!rootEl) {
    renderFatal("页面缺少 #root 容器。");
    return;
  }
  try {
    const handshake = await discoverHandshake();
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
