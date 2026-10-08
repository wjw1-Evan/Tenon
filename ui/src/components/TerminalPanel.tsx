// 项目终端面板（§7.2 v1.197）：xterm.js + 终端 WS——底部面板第四 tab。
// 打开即开终端（幂等，daemon 侧项目级单实例）；fit 插件同步尺寸
//（resize 经 JSON 控制帧）；卸载 / 关 tab 不杀终端进程（仅断流，
// 杀进程走 DELETE 端点 / 项目运行时关闭）。
import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

export function TerminalPanel({
  api,
  projectId,
  t,
}: {
  api: TenonApi;
  projectId: string | null;
  t: Translate;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const termRef = useRef<Terminal | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!projectId || !host) return;
    let disposed = false;
    let ws: WebSocket | null = null;
    const term = new Terminal({
      fontSize: 12,
      convertEol: false,
      theme: { background: "#111318" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    termRef.current = term;

    const doFit = () => {
      try {
        fit.fit();
      } catch {
        // 面板隐藏时尺寸为 0：跳过本次同步
      }
    };
    doFit();

    let active = true;
    api
      .openTerminal(projectId)
      .then(() => {
        if (!active) return null;
        return api.connectTerminal(
          projectId,
          (data) => {
            if (typeof data === "string") term.write(data);
            else term.write(data);
          },
          () => {
            if (!disposed) term.writeln("");
          }
        );
      })
      .then((socket) => {
        if (!active || !socket) {
          socket?.close();
          return;
        }
        ws = socket;
        term.onData((data) => {
          if (ws && ws.readyState === WebSocket.OPEN) ws.send(data);
        });
        // 尺寸同步（fit 后 + 容器尺寸变化）
        const sendResize = () => {
          if (ws && ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ resize: [term.rows, term.cols] }));
          }
        };
        sendResize();
        const observer = new ResizeObserver(() => {
          doFit();
          sendResize();
        });
        observer.observe(host);
        term.focus();
      })
      .catch(() => {
        if (!disposed) term.writeln(t("terminal.open_failed"));
      });

    return () => {
      disposed = true;
      active = false;
      ws?.close();
      term.dispose();
      termRef.current = null;
    };
  }, [api, projectId, t]);

  return (
    <div className="terminal-panel" data-testid="terminal-panel">
      <div
        ref={hostRef}
        className="terminal-host"
        style={{ width: "100%", height: "100%" }}
      />
    </div>
  );
}
