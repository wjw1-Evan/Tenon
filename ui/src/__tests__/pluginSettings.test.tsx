// 插件管理分区（§13 / v1.84）：已装清单、registry 检索与 D 级两阶段安装。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PluginSettings } from "../components/PluginSettings";
import type { TenonApi } from "../lib/api";

const installed = [
  {
    id: "community.existing",
    version: "1.0.0",
    permissions: ["fs.read:project"],
    installed_at: "2026-01-01T00:00:00Z",
  },
];

const hit = {
  id: "community.demo",
  version: "2.0.0",
  sha256: "a".repeat(64),
  signature: "b".repeat(64),
  url: "https://registry.local/demo.yaml",
  description: "Demo plugin",
};

function makeApi() {
  return {
    listPlugins: vi.fn().mockResolvedValue({ installed }),
    searchPlugins: vi.fn().mockResolvedValue({ hits: [hit] }),
    installPlugin: vi
      .fn()
      .mockImplementationOnce(() =>
        Promise.resolve({
          approval_id: "approval-1",
          level: "d",
          permission_diff: {
            added: ["net:registry:npm"],
            removed: [],
            unchanged: ["fs.read:project"],
          },
          manifest: { id: hit.id, version: hit.version },
        })
      )
      .mockImplementationOnce(() =>
        Promise.resolve({ installed: true, id: hit.id, version: hit.version })
      ),
    decideApproval: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

const t = (key: string) => key;

describe("PluginSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders installed plugins and permissions", async () => {
    render(<PluginSettings api={makeApi()} t={t} />);
    await waitFor(() =>
      expect(screen.getByText("community.existing")).toBeTruthy()
    );
    expect(screen.getByText("fs.read:project")).toBeTruthy();
  });

  it("searches and completes two-stage D-level installation after approval", async () => {
    const api = makeApi();
    render(<PluginSettings api={api} t={t} />);
    await waitFor(() => expect(screen.getByText("community.existing")).toBeTruthy());

    fireEvent.change(screen.getByTestId("plugin-search"), {
      target: { value: "demo" },
    });
    fireEvent.click(screen.getByTestId("plugin-search-button"));
    await waitFor(() => expect(screen.getByText("Demo plugin")).toBeTruthy());

    fireEvent.click(screen.getByTestId("plugin-install-community.demo"));
    await waitFor(() => expect(screen.getByTestId("plugin-permission-diff")).toBeTruthy());
    expect(screen.getByText("+ net:registry:npm")).toBeTruthy();
    expect(screen.getByText("= fs.read:project")).toBeTruthy();

    fireEvent.click(screen.getByTestId("plugin-approve-install"));
    await waitFor(() => expect(api.decideApproval).toHaveBeenCalledWith("approval-1", "once"));
    await waitFor(() =>
      expect(api.installPlugin).toHaveBeenNthCalledWith(
        2,
        hit,
        [],
        "approval-1"
      )
    );
    await waitFor(() => expect(screen.getByText("community.demo")).toBeTruthy());
  });
});
