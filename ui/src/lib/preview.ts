// 预览目标解析（§7.6 v2.77）：预览面板里用户输入的 dev server 地址 → 反代目标。
// 约束与 daemon 侧 `preview.rs` 同源——**仅回环主机 + 显式端口**，其余一律判非法
//（反代只允许 127.0.0.1，前端提前拦掉才能给出可读提示而不是 400/502）。

export interface PreviewTarget {
  /** dev server 监听端口（反代路径中的 `{port}` 段）。 */
  port: number;
  /** 归一化地址（「在浏览器打开」用），恒为 127.0.0.1 形式。 */
  url: string;
  /** 代理路径尾段（含首斜杠），直接拼在 daemon 前缀之后。 */
  path: string;
}

const LOOPBACK = new Set(["127.0.0.1", "localhost", "::1"]);

/** 解析用户输入：`3000` / `:3000` / `localhost:3000` / `http://127.0.0.1:3000/x` 均可。 */
export function parsePreviewTarget(raw: string): PreviewTarget | null {
  const input = raw.trim();
  if (!input) return null;
  let normalized = input;
  if (/^\d+$/.test(input)) normalized = `http://127.0.0.1:${input}`;
  else if (/^:\d+$/.test(input)) normalized = `http://127.0.0.1${input}`;
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(normalized)) normalized = `http://${normalized}`;

  let parsed: URL;
  try {
    parsed = new URL(normalized);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  if (!parsed.port) return null;
  const host = parsed.hostname.replace(/^\[/, "").replace(/\]$/, "");
  if (!LOOPBACK.has(host)) return null;
  const port = Number(parsed.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
  const path = `${parsed.pathname || "/"}${parsed.search}`;
  return { port, url: `http://127.0.0.1:${port}${path}`, path };
}

/** 反代 iframe 地址：daemon 前缀（已以 `/` 结尾）+ 端口 + 路径。 */
export function previewFrameUrl(
  origin: string,
  prefix: string,
  target: PreviewTarget
): string {
  return `${origin}${prefix}${target.port}${target.path}`;
}
