// 更新日志随包展示（v1.152 / §6.2）：壳环境 peek→弹窗→dismiss 消费链、
// 浏览器 Web 版不渲染、发布说明行轻量可读化。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UpdateNotesDialog, noteLines, type UpdateNotes } from "../components/UpdateNotesDialog";

// 假翻译器与真 createTranslator 同语义：查表取值后做 {var} 插值（键本身不含占位符）
const VALUES: Record<string, string> = {
  "update.notes.title": "Tenon 已更新到 {version}",
  "update.notes.body_title": "更新日志",
  "update.notes.empty": "本次更新未提供详细说明。",
  "update.notes.view_full": "在 GitHub 查看完整发布说明",
  "update.notes.dismiss": "知道了",
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

describe("UpdateNotesDialog", () => {
  it("浏览器 Web 版（无 __TAURI_INTERNALS__）不渲染", () => {
    render(<UpdateNotesDialog t={t} />);
    expect(screen.queryByTestId("update-notes-overlay")).not.toBeInTheDocument();
  });

  it("壳环境 peek 到随包日志：渲染标题与 • 列表，重载仍可见（peek 不消费）", async () => {
    const invoke = vi.fn().mockResolvedValue({
      version: "0.2.0",
      notes: "- Apple 签名与公证链路首用\n* CI 收尾根治 ETXTBSY\n\nTenon v0.2.0",
    } satisfies UpdateNotes | null);
    stubTauri(invoke);
    render(<UpdateNotesDialog t={t} />);
    expect(await screen.findByTestId("update-notes-overlay")).toBeInTheDocument();
    expect(screen.getByTestId("update-notes-title")).toHaveTextContent(
      "Tenon 已更新到 0.2.0"
    );
    const items = document.querySelectorAll(".update-notes-item");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("Apple 签名与公证链路首用");
    expect(document.querySelectorAll(".update-notes-line")).toHaveLength(1);
    // peek 语义：只调过 get，未调 dismiss（invoke 第二参 args 常规传 undefined）
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("get_update_notes", undefined);
  });

  it("手动升级（notes 为空）渲染占位说明；「知道了」走 dismiss 并消失", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce({ version: "0.2.0", notes: null })
      .mockResolvedValueOnce(undefined);
    stubTauri(invoke);
    render(<UpdateNotesDialog t={t} />);
    expect(await screen.findByTestId("update-notes-empty")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("update-notes-dismiss"));
    await waitFor(() => {
      expect(screen.queryByTestId("update-notes-overlay")).not.toBeInTheDocument();
    });
    expect(invoke).toHaveBeenCalledWith("dismiss_update_notes", undefined);
  });

  it("完整发布说明外链经 shell opener 打开 Release 页", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce({ version: "0.2.0", notes: "- a" })
      .mockResolvedValueOnce(undefined);
    stubTauri(invoke);
    render(<UpdateNotesDialog t={t} />);
    fireEvent.click(await screen.findByTestId("update-notes-link"));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("plugin:shell|open", {
        path: "https://github.com/wjw1-Evan/Tenon/releases/tag/v0.2.0",
      });
    });
  });

  it("peek 返回 null（无待展示更新）不渲染弹窗", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    stubTauri(invoke);
    render(<UpdateNotesDialog t={t} />);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalled();
    });
    expect(screen.queryByTestId("update-notes-overlay")).not.toBeInTheDocument();
  });
});

describe("noteLines", () => {
  it("列表行转 • 前缀，空行过滤，其余原样", () => {
    expect(noteLines("- 甲\n* 乙\n\n普通行\n")).toEqual([
      { bullet: true, text: "甲" },
      { bullet: true, text: "乙" },
      { bullet: false, text: "普通行" },
    ]);
  });
});
