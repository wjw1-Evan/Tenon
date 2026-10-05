// 语言包向导测试：项目感知推荐 / 一键安装 / 已就绪状态。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguagePackWizard } from "../components/LanguagePackWizard";
import type { TenonApi } from "../lib/api";

const detectMock = vi.fn();
const installMock = vi.fn();

function api() {
  return { detectLanguagePacks: detectMock, installLanguagePack: installMock } as unknown as TenonApi;
}

beforeEach(() => {
  detectMock.mockReset();
  installMock.mockReset();
});

afterEach(cleanup);

describe("LanguagePackWizard", () => {
  it("returns null when no projectId", () => {
    const { container } = render(<LanguagePackWizard api={api()} projectId={null} />);
    expect(container.querySelector(".lp-wizard")).toBeNull();
    expect(detectMock).not.toHaveBeenCalled();
  });

  it("renders detected packs with install buttons for missing servers", async () => {
    detectMock.mockResolvedValue({
      packs: [
        { language: "typescript", command: "npx", detected: true, server_installed: true, runtime_hint: null },
        { language: "python", command: "pip", detected: true, server_installed: false, runtime_hint: "需要 pyright" },
      ],
    });
    render(<LanguagePackWizard api={api()} projectId="p1" />);

    await waitFor(() => expect(detectMock).toHaveBeenCalledWith("p1"));
    expect(screen.getByTestId("lp-typescript")).toBeTruthy();
    expect(screen.getByText("已就绪")).toBeTruthy();
    expect(screen.getByTestId("lp-install-python")).toBeTruthy();
    expect(screen.getByText("需要 pyright")).toBeTruthy();
  });

  it("marks a pack as installed after clicking install", async () => {
    detectMock.mockResolvedValue({
      packs: [
        { language: "rust", command: "cargo", detected: true, server_installed: false, runtime_hint: "rust-analyzer" },
      ],
    });
    installMock.mockResolvedValue({ installed: true });
    const onInstalled = vi.fn();
    render(<LanguagePackWizard api={api()} projectId="p2" onInstalled={onInstalled} />);

    await waitFor(() => expect(screen.getByTestId("lp-install-rust")).toBeTruthy());
    fireEvent.click(screen.getByTestId("lp-install-rust"));
    await waitFor(() => expect(installMock).toHaveBeenCalledWith("p2", "rust"));
    expect(await screen.findByText("刚安装 ✓")).toBeTruthy();
    expect(onInstalled).toHaveBeenCalledWith("rust");
  });

  it("hides entirely when no packs are returned", async () => {
    detectMock.mockResolvedValue({ packs: [] });
    const { container } = render(<LanguagePackWizard api={api()} projectId="p3" />);
    await waitFor(() => expect(detectMock).toHaveBeenCalled());
    expect(container.querySelector(".lp-wizard")).toBeNull();
  });
});
