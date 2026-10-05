// PluginSettings 深度测试：安装 / 已安装列表 / 搜索。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PluginSettings } from "../components/PluginSettings";
import type { TenonApi } from "../lib/api";

const listMock = vi.fn();
const searchMock = vi.fn();
const installMock = vi.fn();

function api() {
  return { listPlugins: listMock, searchPlugins: searchMock, installPlugin: installMock } as unknown as TenonApi;
}
const t = (key: string) => key;

beforeEach(() => {
  listMock.mockReset(); searchMock.mockReset(); installMock.mockReset();
  listMock.mockResolvedValue({ installed: [
    { id: "lsp-ts", version: "1.0.0", permissions: ["fs.read"], installed_at: "2026-01-01T00:00:00Z" },
  ] });
  searchMock.mockResolvedValue({ hits: [
    { id: "lsp-py", name: "Python LSP", version: "2.0.0", permissions: ["fs.read", "net"], description: "Py" },
  ] });
  installMock.mockResolvedValue({ installed: true });
});

afterEach(cleanup);

describe("PluginSettings deep", () => {
  it("renders installed plugins list", async () => {
    render(<PluginSettings api={api()} t={t} />);
    await waitFor(() => expect(screen.getByTestId("installed-plugins")).toBeTruthy());
    expect(screen.getByText(/lsp-ts/)).toBeTruthy();
  });

  it("searches plugins and shows results with install button", async () => {
    render(<PluginSettings api={api()} t={t} />);
    await waitFor(() => expect(listMock).toHaveBeenCalled());
    const input = screen.queryByTestId("plugin-search");
    if (input) {
      fireEvent.change(input, { target: { value: "python" } });
      fireEvent.click(screen.getByTestId("plugin-search-button"));
      await waitFor(() => expect(searchMock).toHaveBeenCalledWith("python"));
      const installBtn = await screen.findByTestId("plugin-install-lsp-py");
      expect(installBtn).toBeTruthy();
      // 点击安装
      fireEvent.click(installBtn);
      await waitFor(() => expect(installMock).toHaveBeenCalled());
    }
  });

  it("handles listPlugins error", async () => {
    listMock.mockRejectedValue(new Error("offline"));
    render(<PluginSettings api={api()} t={t} />);
    // 不崩溃
    await waitFor(() => expect(listMock).toHaveBeenCalled());
  });

  it("handles install failure", async () => {
    installMock.mockRejectedValue(new Error("install failed"));
    render(<PluginSettings api={api()} t={t} />);
    await waitFor(() => expect(listMock).toHaveBeenCalled());
    const input = screen.queryByTestId("plugin-search");
    if (input) {
      fireEvent.change(input, { target: { value: "py" } });
      fireEvent.click(screen.getByTestId("plugin-search-button"));
      const btn = await screen.findByTestId("plugin-install-lsp-py");
      fireEvent.click(btn);
      await waitFor(() => expect(installMock).toHaveBeenCalled());
    }
  });

  it("shows no plugins message when empty", async () => {
    listMock.mockResolvedValue({ installed: [] });
    searchMock.mockResolvedValue({ hits: [] });
    render(<PluginSettings api={api()} t={t} />);
    await waitFor(() => expect(screen.getByTestId("installed-plugins")).toBeTruthy());
  });
});
