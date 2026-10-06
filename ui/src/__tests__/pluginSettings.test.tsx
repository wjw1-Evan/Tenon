// MCP 插件管理分区（§13.5 / §7.5，v1.145 重构）：已装列表（settings mcp.servers：
// 启停 / 删除 / 来源徽标）+ 手动添加 + 市场子视图（MCP 命令面确认、更新比对）。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PluginSettings } from "../components/PluginSettings";
import type { McpServerData, TenonApi } from "../lib/api";

const servers: Record<string, McpServerData> = {
  github: {
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-github"],
    enabled: true,
    permissions: ["net:*"],
    source: "mocksrc/repo",
    version: "1.0.0",
  },
  localfs: { command: "node", args: ["fs.js"], enabled: false, permissions: [], source: null },
};

function makeApi() {
  return {
    putSettings: vi.fn().mockImplementation((body: Record<string, unknown>) =>
      Promise.resolve({ mcp: body.mcp })
    ),
    getSettings: vi
      .fn()
      .mockResolvedValue({ mcp: { servers: { ...servers } } }),
    listMarketSources: vi.fn().mockResolvedValue({ sources: ["mocksrc/repo"] }),
    putMarketSources: vi.fn().mockResolvedValue({ sources: ["mocksrc/repo"] }),
    getMarketManifest: vi.fn().mockResolvedValue({
      name: "Test Market",
      entries: [
        {
          kind: "mcp",
          name: "github",
          description: "GitHub API",
          command: "npx",
          args: ["-y", "@modelcontextprotocol/server-github"],
          version: "2.0.0",
        },
        { kind: "skill", name: "pdf", description: "PDF", path: "skills/pdf", version: "1.0.0" },
      ],
    }),
    marketInstall: vi.fn().mockResolvedValue({ installed: true, note: "新会话生效" }),
    marketUninstall: vi.fn().mockResolvedValue({ uninstalled: true }),
  } as unknown as TenonApi;
}

const t = (key: string) => key;

function renderPanel(api: TenonApi) {
  render(<PluginSettings api={api} t={t} servers={servers} onSaved={vi.fn()} />);
}

describe("PluginSettings（MCP 插件管理，v1.145）", () => {
  beforeEach(() => {
    vi.stubGlobal("confirm", vi.fn().mockReturnValue(true));
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("renders installed servers with command face, source badge and enabled state", async () => {
    renderPanel(makeApi());
    await waitFor(() => expect(screen.getByTestId("mcp-server-github")).toBeTruthy());
    expect(screen.getByTestId("mcp-server-localfs")).toBeTruthy();
    expect((screen.getByTestId("mcp-toggle-github") as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId("mcp-toggle-localfs") as HTMLInputElement).checked).toBe(false);
    // skill 条目不进 mcp 市场列表。
    await waitFor(() => expect(screen.getByTestId("market-entry-github")).toBeTruthy());
    expect(screen.queryByTestId("market-entry-pdf")).toBeNull();
  });

  it("toggles via PUT /settings mcp.servers whole-map replace", async () => {
    const api = makeApi();
    renderPanel(api);
    await waitFor(() => expect(screen.getByTestId("mcp-toggle-github")).toBeTruthy());
    fireEvent.click(screen.getByTestId("mcp-toggle-github"));
    await waitFor(() => expect(api.putSettings).toHaveBeenCalled());
    const body = (api.putSettings as ReturnType<typeof vi.fn>).mock.calls[0][0] as {
      mcp: { servers: Record<string, McpServerData> };
    };
    expect(body.mcp.servers.github.enabled).toBe(false);
    expect(body.mcp.servers.localfs).toBeTruthy();
  });

  it("removes an entry after confirm", async () => {
    const api = makeApi();
    renderPanel(api);
    await waitFor(() => expect(screen.getByTestId("mcp-remove-localfs")).toBeTruthy());
    fireEvent.click(screen.getByTestId("mcp-remove-localfs"));
    await waitFor(() => expect(api.putSettings).toHaveBeenCalled());
    const body = (api.putSettings as ReturnType<typeof vi.fn>).mock.calls[0][0] as {
      mcp: { servers: Record<string, McpServerData> };
    };
    expect(body.mcp.servers.localfs).toBeUndefined();
    expect(body.mcp.servers.github).toBeTruthy();
  });

  it("adds a manual server with launcher command and net permission", async () => {
    const api = makeApi();
    renderPanel(api);
    await waitFor(() => expect(screen.getByTestId("mcp-add")).toBeTruthy());
    fireEvent.change(screen.getByTestId("mcp-new-name"), { target: { value: "search" } });
    fireEvent.change(screen.getByTestId("mcp-new-command"), { target: { value: "uvx" } });
    fireEvent.change(screen.getByTestId("mcp-new-args"), { target: { value: "mcp-server-fetch" } });
    fireEvent.click(screen.getByTestId("mcp-new-net"));
    fireEvent.click(screen.getByTestId("mcp-add"));
    await waitFor(() => expect(api.putSettings).toHaveBeenCalled());
    const body = (api.putSettings as ReturnType<typeof vi.fn>).mock.calls[0][0] as {
      mcp: { servers: Record<string, McpServerData> };
    };
    expect(body.mcp.servers.search).toMatchObject({
      command: "uvx",
      args: ["mcp-server-fetch"],
      permissions: ["net:*"],
      enabled: true,
    });
  });

  it("market install confirms the command face and refreshes settings; update state by version", async () => {
    const api = makeApi();
    const onSaved = vi.fn();
    render(<PluginSettings api={api} t={t} servers={servers} onSaved={onSaved} />);
    await waitFor(() => expect(screen.getByTestId("market-entry-github")).toBeTruthy());
    // 已装 1.0.0 vs 清单 2.0.0 → 更新态。
    expect(screen.getByTestId("market-install-github").textContent).toBe(
      "settings.market.update"
    );
    fireEvent.click(screen.getByTestId("market-install-github"));
    await waitFor(() => expect(api.marketInstall).toHaveBeenCalled());
    const call = (api.marketInstall as ReturnType<typeof vi.fn>).mock.calls[0] as string[];
    expect(call).toEqual(["mocksrc/repo", "mcp", "github"]);
    // 确认框展示命令行面。
    const confirm = vi.mocked(window.confirm);
    expect(confirm.mock.calls[0]?.[0]).toContain("npx");
    // 安装成功后重拉设置回写。
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
  });

  it("market uninstall via row ✕", async () => {
    const api = makeApi();
    renderPanel(api);
    await waitFor(() => expect(screen.getByTestId("market-uninstall-github")).toBeTruthy());
    fireEvent.click(screen.getByTestId("market-uninstall-github"));
    await waitFor(() =>
      expect(api.marketUninstall).toHaveBeenCalledWith("mcp", "github")
    );
  });
});
