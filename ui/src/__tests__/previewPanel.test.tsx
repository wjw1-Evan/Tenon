// 浏览器预览面板测试（§7.6 v2.77）：地址解析（回环 + 显式端口约束）、打开链路
//（取反代前缀 → iframe 地址带频道号）、控制台桥按频道归户、非法地址与令牌失败
// 的可见提示、项目切换回填记忆地址。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, act } from "@testing-library/react";
import { PreviewPanel } from "../components/PreviewPanel";
import { parsePreviewTarget, previewFrameUrl } from "../lib/preview";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
});

const t: Translate = ((key: string, vars?: Record<string, unknown>) => {
  if (key === "preview.invalid") return "INVALID";
  if (key === "preview.unavailable") return "UNAVAILABLE";
  if (key === "preview.empty") return "EMPTY";
  if (key === "preview.logs_empty") return "LOGS_EMPTY";
  return vars ? `${key}:${JSON.stringify(vars)}` : key;
}) as Translate;

function makeApi(opts?: { failInfo?: boolean }) {
  const raw = {
    previewInfo: vi.fn().mockImplementation(async () => {
      if (opts?.failInfo) throw new Error("401");
      return { origin: "http://127.0.0.1:9876", prefix: "/preview/SEC/proxy/" };
    }),
  };
  return { api: raw as unknown as TenonApi, mocks: raw };
}

function postPreviewMessage(data: Record<string, unknown>) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data }));
  });
}

async function open(
  api: TenonApi,
  input: string,
  projectId: string | null = "p1"
) {
  render(<PreviewPanel api={api} projectId={projectId} t={t} />);
  await act(async () => {
    fireEvent.change(screen.getByTestId("preview-url"), { target: { value: input } });
    fireEvent.click(screen.getByTestId("preview-go"));
  });
}

function frameSrc(): string {
  return screen.getByTestId("preview-frame").getAttribute("src") ?? "";
}

describe("parsePreviewTarget（§7.6 反代目标约束）", () => {
  it("接受裸端口 / 带 scheme / 带路径的多种写法", () => {
    expect(parsePreviewTarget("3000")).toEqual({
      port: 3000,
      url: "http://127.0.0.1:3000/",
      path: "/",
    });
    expect(parsePreviewTarget(":5173")).toEqual({
      port: 5173,
      url: "http://127.0.0.1:5173/",
      path: "/",
    });
    expect(parsePreviewTarget("localhost:4000/admin?a=1")).toEqual({
      port: 4000,
      url: "http://127.0.0.1:4000/admin?a=1",
      path: "/admin?a=1",
    });
    expect(parsePreviewTarget("http://127.0.0.1:8000/")?.port).toBe(8000);
  });

  it("拒绝非回环、缺端口与非法输入（daemon 侧同样只允许回环）", () => {
    for (const bad of [
      "",
      "example.com:3000",
      "192.168.1.5:3000",
      "http://10.0.0.1:8080",
      "localhost",
      "http://127.0.0.1",
      "file:///etc/passwd",
      "3000abc",
    ]) {
      expect(parsePreviewTarget(bad), bad).toBeNull();
    }
  });

  it("iframe 地址 = daemon 源 + 反代前缀 + 端口 + 路径", () => {
    const target = parsePreviewTarget("3000")!;
    expect(previewFrameUrl("http://127.0.0.1:9876", "/preview/SEC/proxy/", target)).toBe(
      "http://127.0.0.1:9876/preview/SEC/proxy/3000/"
    );
  });
});

describe("PreviewPanel（§7.6 预览面板）", () => {
  it("空态：未填地址时给出引导，不请求反代信息", () => {
    const { api, mocks } = makeApi();
    render(<PreviewPanel api={api} projectId="p1" t={t} />);
    expect(screen.getByTestId("preview-empty").textContent).toBe("EMPTY");
    expect(mocks.previewInfo).not.toHaveBeenCalled();
  });

  it("打开链路：取反代前缀 → iframe 指向代理地址且带频道号", async () => {
    const { api, mocks } = makeApi();
    await open(api, "localhost:5173/app");

    await waitFor(() => expect(mocks.previewInfo).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByTestId("preview-frame")).toBeTruthy());
    const src = new URL(frameSrc());
    expect(src.origin + src.pathname).toBe(
      "http://127.0.0.1:9876/preview/SEC/proxy/5173/app"
    );
    expect(src.searchParams.get("__tenon")).toBeTruthy();
    cleanup();
  });

  it("控制台桥：同频道消息入面板、异频道忽略", async () => {
    const { api } = makeApi();
    await open(api, "3000");
    await waitFor(() => expect(screen.queryByTestId("preview-frame")).toBeTruthy());
    const channel = new URL(frameSrc()).searchParams.get("__tenon")!;
    expect(screen.getByTestId("preview-console").textContent).toBe("LOGS_EMPTY");

    postPreviewMessage({
      __tenon_preview: true,
      channel: "other-panel",
      kind: "console",
      level: "error",
      text: "不该出现",
    });
    expect(screen.queryByTestId("preview-log-error")).toBeNull();

    postPreviewMessage({
      __tenon_preview: true,
      channel,
      kind: "console",
      level: "error",
      text: "boom",
    });
    postPreviewMessage({
      __tenon_preview: true,
      channel,
      kind: "console",
      level: "log",
      text: "hi",
    });
    expect(await screen.findByTestId("preview-log-error")).toHaveTextContent("boom");
    expect(screen.getByTestId("preview-log-log")).toHaveTextContent("hi");

    // 非本桥负载（页面自身 postMessage）一律忽略
    postPreviewMessage({ kind: "console", channel, text: "无桥标记" });
    expect(screen.queryAllByTestId("preview-log-log")).toHaveLength(1);

    fireEvent.click(screen.getByTestId("preview-console-clear"));
    expect(screen.queryByTestId("preview-log-error")).toBeNull();
    cleanup();
  });

  it("非法地址：给可读提示且不渲染 iframe；反代不可用：提示取令牌失败", async () => {
    const { api } = makeApi();
    await open(api, "192.168.1.5:3000");
    expect(await screen.findByTestId("preview-error")).toHaveTextContent("INVALID");
    expect(screen.queryByTestId("preview-frame")).toBeNull();
    cleanup();

    const failing = makeApi({ failInfo: true });
    await open(failing.api, "3000");
    expect(await screen.findByTestId("preview-error")).toHaveTextContent("UNAVAILABLE");
    cleanup();
  });

  it("记忆地址：打开成功后落盘，重挂载同一项目回填并自动预览", async () => {
    const { api, mocks } = makeApi();
    await open(api, "127.0.0.1:4321");
    await waitFor(() => expect(screen.queryByTestId("preview-frame")).toBeTruthy());
    cleanup();

    const again = makeApi();
    render(<PreviewPanel api={again.api} projectId="p1" t={t} />);
    await act(async () => {});
    expect((screen.getByTestId("preview-url") as HTMLInputElement).value).toBe(
      "127.0.0.1:4321"
    );
    await waitFor(() => expect(again.mocks.previewInfo).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByTestId("preview-frame")).toBeTruthy());
    expect(frameSrc()).toContain("/preview/SEC/proxy/4321/");
  });
});
