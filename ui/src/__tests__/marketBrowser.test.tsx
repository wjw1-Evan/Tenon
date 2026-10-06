// 市场子视图测试（§13.5 / v1.149）：源拉取失败可见可重试——失败徽标取代静默
// 「…」计数，重试成功恢复条目计数、仍失败刷新错误态；卸载与安装态联动。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MarketBrowser } from "../components/MarketBrowser";
import type { TenonApi } from "../lib/api";

const t = (key: string) => key;

function makeApi(
  manifestImpl: (source: string) => Promise<unknown>,
  sources: string[] = ["bad/repo", "good/repo"]
) {
  return {
    listMarketSources: vi.fn().mockResolvedValue({ sources }),
    putMarketSources: vi.fn().mockResolvedValue({ sources }),
    getMarketManifest: vi.fn().mockImplementation((owner: string, repo: string) =>
      manifestImpl(`${owner}/${repo}`)
    ),
    marketInstall: vi.fn().mockResolvedValue({ installed: true }),
    marketUninstall: vi.fn().mockResolvedValue({ uninstalled: true }),
  } as unknown as TenonApi;
}

const okManifest = {
  name: "Good Market",
  entries: [{ kind: "skill", name: "pdf", description: "PDF", path: "skills/pdf" }],
};

function renderBrowser(api: TenonApi) {
  render(<MarketBrowser api={api} t={t} kind="skill" installed={{}} onChanged={() => {}} />);
}

describe("MarketBrowser 源拉取失败可见可重试（v1.149）", () => {
  afterEach(cleanup);

  it("失败源显示「加载失败」徽标而非静默「…」；成功源正常计数", async () => {
    renderBrowser(makeApi((source) => (source === "bad/repo" ? Promise.reject(new Error("镜像链全部失败")) : Promise.resolve(okManifest))));
    await waitFor(() => expect(screen.getByTestId("market-failed-bad/repo")).toBeTruthy());
    expect(screen.getByTestId("market-failed-bad/repo").textContent).toBe(
      "settings.market.load_failed"
    );
    // 重试钮出现。
    expect(screen.getByTestId("market-retry-bad/repo").textContent).toBe("settings.market.retry");
    // 成功源不受影响（§13.5：单源失败不阻塞其余）。
    await waitFor(() =>
      expect(screen.getByTestId("market-list-good/repo")).toBeTruthy()
    );
  });

  it("重试成功：失败态清除、条目计数恢复", async () => {
    let fail = true;
    const api = makeApi((source) =>
      source === "bad/repo" && fail
        ? Promise.reject(new Error("down"))
        : Promise.resolve(okManifest)
    );
    renderBrowser(api);
    await waitFor(() => expect(screen.getByTestId("market-failed-bad/repo")).toBeTruthy());
    fail = false;
    fireEvent.click(screen.getByTestId("market-retry-bad/repo"));
    await waitFor(() =>
      expect(screen.queryByTestId("market-failed-bad/repo")).toBeNull()
    );
    expect(screen.getByTestId("market-list-bad/repo")).toBeTruthy();
  });

  it("重试仍失败：失败态保留（错误文案刷新）", async () => {
    const api = makeApi(() => Promise.reject(new Error("still down")));
    renderBrowser(api);
    await waitFor(() => expect(screen.getByTestId("market-failed-bad/repo")).toBeTruthy());
    fireEvent.click(screen.getByTestId("market-retry-bad/repo"));
    await waitFor(() =>
      expect(
        (screen.getByTestId("market-failed-bad/repo") as HTMLElement).title
      ).toContain("still down")
    );
    expect(screen.getByTestId("market-retry-bad/repo")).toBeTruthy();
  });
});
