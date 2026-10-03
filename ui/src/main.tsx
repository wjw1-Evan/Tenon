import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

// M0：桌面壳（Tauri sidecar）注入握手参数；浏览器开发用默认值。
// 真实握手行来自 `tenon-daemon` stdout（{"tenon":1,"port":…,"token":…}）。
declare global {
  interface Window {
    __TENON_HANDSHAKE__?: { port: number; token: string };
    __TENON_PROJECT__?: string;
  }
}

const handshake =
  window.__TENON_HANDSHAKE__ ??
  (() => {
    const params = new URLSearchParams(location.search);
    const port = params.get("port");
    const token = params.get("token");
    if (port && token) return { port: Number(port), token };
    // 开发默认（daemon 手动启动后填入或经 ?port=&token= 传入）
    return { port: 9876, token: "dev" };
  })();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App
      handshake={handshake}
      projectPath={window.__TENON_PROJECT__ ?? new URLSearchParams(location.search).get("project") ?? "."}
    />
  </StrictMode>
);
