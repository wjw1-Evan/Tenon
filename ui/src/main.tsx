import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

// 握手来源优先级：
// 1. Tauri 桌面壳注入 `window.__TENON_HANDSHAKE__`
// 2. URL 查询参数 `?port=&token=`
// 3. 同源自发现：fetch /pairing（daemon 同源托管 UI 时自动获取）
declare global {
  interface Window {
    __TENON_HANDSHAKE__?: { port: number; token: string };
    __TENON_PROJECT__?: string;
  }
}

interface Handshake {
  port: number;
  token: string;
}

function fromParams(): Handshake | null {
  const params = new URLSearchParams(location.search);
  const port = params.get("port");
  const token = params.get("token");
  if (port && token) return { port: Number(port), token };
  return null;
}

async function discoverHandshake(): Promise<Handshake> {
  // 1. Tauri 注入
  if (window.__TENON_HANDSHAKE__) return window.__TENON_HANDSHAKE__;
  // 2. URL 查询参数
  const fromQuery = fromParams();
  if (fromQuery) return fromQuery;
  // 3. 同源自发现：fetch /pairing（本机回环请求，daemon 返回完整握手）
  try {
    const resp = await fetch("/pairing");
    if (resp.ok) {
      const d = await resp.json();
      if (d.port && d.token) return { port: d.port, token: d.token };
    }
  } catch {
    // 不可达
  }
  // 开发默认
  return { port: 9876, token: "dev" };
}

function projectPath(): string {
  return (
    window.__TENON_PROJECT__ ??
    new URLSearchParams(location.search).get("project") ??
    "."
  );
}

async function boot() {
  const handshake = await discoverHandshake();
  const root = createRoot(document.getElementById("root")!);
  root.render(
    <StrictMode>
      <App handshake={handshake} projectPath={projectPath()} />
    </StrictMode>
  );
}

boot();
