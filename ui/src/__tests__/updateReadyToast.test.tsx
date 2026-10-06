// 更新就绪通知卡（v1.154 / §6.2）：壳环境 peek→通知卡→install 链、
// 浏览器 Web 版不渲染、「下次再说」仅收起本次。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UpdateReadyToast } from "../components/UpdateReadyToast";

// 假翻译器与真 createTranslator 同语义：查表取值后做 {var} 插值
const VALUES: Record<string, string> = {
  "update.ready.title": "已下载更新 v{version}",
  "update.ready.install": "立即更新",
  "update.ready.later": "下次再说",
  "update.notes.empty": "本次更新未提供详细说明。",
  "update.notes.view_full": "在 GitHub 查看完整发布说明",
};
const t = (key: string, vars?: Record<string, string | number>) => {
  let text = VALUES[key] ?? key;
  for (const [k, v] of Object.entries(vars ?? {})) text = text.replaceAll(`{${k}}`, String(v));
  return text;
};

function stubTauri(invoke: ReturnType<typeof vi.fn>) {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = { invoke };
}

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

describe("UpdateReadyToast", () => {
  it("浏览器 Web 版（无 __TAURI_INTERNALS__）不渲染", () => {
    render(<UpdateReadyToast t={t} />);
    expect(screen.queryByTestId("update-ready-toast")).not.toBeInTheDocument();
  });

  it("壳环境 peek 到已下载更新：渲染标题与 • 列表（peek 不消费）", async () => {
    const invoke = vi.fn().mockResolvedValue({
      version: "0.2.0",
      notes: "- 更新固定自动\n* 下载完成用户确认重启",
    });
    stubTauri(invoke);
    render(<UpdateReadyToast t={t} />);
    expect(await screen.findByTestId("update-ready-toast")).toBeInTheDocument();
    expect(screen.getByTestId("update-ready-title")).toHaveTextContent("已下载更新 v0.2.0");
    expect(document.querySelectorAll(".update-notes-item")).toHaveLength(2);
    expect(invoke).toHaveBeenCalledWith("get_update_ready", undefined);
  });

  it("notes 为空渲染占位说明；「下次再说」仅收起不调 IPC", async () => {
    const invoke = vi.fn().mockResolvedValue({ version: "0.2.0", notes: null });
    stubTauri(invoke);
    render(<UpdateReadyToast t={t} />);
    expect(await screen.findByText("本次更新未提供详细说明。")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("update-ready-later"));
    expect(screen.queryByTestId("update-ready-toast")).not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("「立即更新」触发 install_update；失败回落展示原因且按钮恢复", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce({ version: "0.2.0", notes: "- a" })
      .mockRejectedValueOnce(new Error("更新安装失败: boom"));
    stubTauri(invoke);
    render(<UpdateReadyToast t={t} />);
    fireEvent.click(await screen.findByTestId("update-ready-install"));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("install_update", undefined);
    });
    expect(await screen.findByTestId("update-ready-failed")).toHaveTextContent("更新安装失败");
    expect(screen.getByTestId("update-ready-install").hasAttribute("disabled")).toBe(false);
  });

  it("完整发布说明外链经 shell opener 打开 Release 页", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce({ version: "0.2.0", notes: "- a" })
      .mockResolvedValueOnce(undefined);
    stubTauri(invoke);
    render(<UpdateReadyToast t={t} />);
    fireEvent.click(await screen.findByTestId("update-ready-link"));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("plugin:shell|open", {
        path: "https://github.com/wjw1-Evan/Tenon/releases/tag/v0.2.0",
      });
    });
  });

  it("peek 返回 null（无已下载更新）不渲染", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    stubTauri(invoke);
    render(<UpdateReadyToast t={t} />);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalled();
    });
    expect(screen.queryByTestId("update-ready-toast")).not.toBeInTheDocument();
  });
});
