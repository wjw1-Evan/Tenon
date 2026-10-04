// 设置面板（§7.2 / §15）：渲染回填 + 保存载荷 + 校验错误展示。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { SettingsDialog, type SettingsData } from "../components/SettingsDialog";
import type { TenonApi } from "../lib/api";

vi.mock("@monaco-editor/react", () => ({ default: () => null }));

const settings: SettingsData = {
  session: { mode: "interactive", first_edit_buffer_ms: 2000, approval_timeout_s: 120 },
  exec: { command_timeout_s: 120 },
};

function makeApi(put: ReturnType<typeof vi.fn> = vi.fn()) {
  return {
    putSettings: put,
  } as unknown as TenonApi;
}

const t = (k: string) => k;

describe("SettingsDialog", () => {
  beforeEach(() => {
    const backing = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
      setItem: (k: string, v: string) => void backing.set(k, v),
      removeItem: (k: string) => void backing.delete(k),
      clear: () => backing.clear(),
    });
  });
  afterEach(() => vi.unstubAllGlobals());

  it("从全局设置回填表单", () => {
    render(
      <SettingsDialog api={makeApi()} t={t} settings={settings} onClose={() => {}} onSaved={() => {}} />
    );
    const mode = screen.getByLabelText("settings.mode") as HTMLSelectElement;
    expect(mode.value).toBe("interactive");
    const buffer = screen.getByLabelText("settings.buffer") as HTMLInputElement;
    expect(buffer.value).toBe("2000");
  });

  it("保存：载荷含会话与代理参数，成功后回调并关闭", async () => {
    const put = vi.fn().mockResolvedValue(settings);
    const onSaved = vi.fn();
    const onClose = vi.fn();
    render(
      <SettingsDialog api={makeApi(put)} t={t} settings={settings} onClose={onClose} onSaved={onSaved} />
    );
    fireEvent.change(screen.getByLabelText("settings.mode"), {
        target: { value: "auto" },
      });
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(put).toHaveBeenCalledWith(
      expect.objectContaining({
        session: expect.objectContaining({ mode: "auto" }),
        exec: expect.objectContaining({ command_timeout_s: 120 }),
      })
    );
    expect(onClose).toHaveBeenCalled();
  });

  it("保存失败：错误展示且不关闭", async () => {
    const put = vi.fn().mockRejectedValue(new Error("400 非法值"));
    const onClose = vi.fn();
    render(
      <SettingsDialog api={makeApi(put)} t={t} settings={settings} onClose={onClose} onSaved={() => {}} />
    );
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(onClose).not.toHaveBeenCalled();
  });
});
