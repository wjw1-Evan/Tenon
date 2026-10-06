// 技能管理分区（§13.4 / v1.130）：合并清单渲染、启停（settings.skills.disabled）、
// SKILL.md 源码编辑保存、新建（frontmatter 模板）与删除（confirm 门）。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SkillsSettings } from "../components/SkillsSettings";
import type { TenonApi } from "../lib/api";

const skills = [
  {
    name: "commit-helper",
    display_name: "提交助手",
    description: "生成中文提交信息",
    scope: "global",
    dir: "/tenon/skills/commit-helper",
    enabled: true,
  },
  {
    name: "api-mig",
    display_name: "api-mig",
    description: "本项目 API 迁移规范",
    scope: "project",
    dir: "/ws/.tenon/skills/api-mig",
    enabled: false,
  },
];

function makeApi() {
  return {
    listSkills: vi.fn().mockResolvedValue({ skills }),
    getSkill: vi.fn().mockResolvedValue({
      name: "commit-helper",
      scope: "global",
      path: "/tenon/skills/commit-helper/SKILL.md",
      content: "---\nname: 提交助手\ndescription: 生成中文提交信息\n---\n正文",
    }),
    updateSkill: vi.fn().mockResolvedValue({ ok: true, name: "commit-helper" }),
    createSkill: vi.fn().mockResolvedValue({ ok: true, name: "note-writer" }),
    deleteSkill: vi.fn().mockResolvedValue({ ok: true, name: "commit-helper" }),
    putSettings: vi.fn().mockResolvedValue({ skills: { disabled: ["commit-helper"] } }),
    writeFile: vi.fn().mockResolvedValue({ ok: true, created: false }),
    fileOps: vi.fn().mockResolvedValue({ results: [] }),
    // 市场子视图（§13.5 v1.145）：默认空源，不干扰本文件既有用例
    listMarketSources: vi.fn().mockResolvedValue({ sources: [] }),
    putMarketSources: vi.fn().mockResolvedValue({ sources: [] }),
    getMarketManifest: vi.fn().mockResolvedValue({ entries: [] }),
    marketInstall: vi.fn().mockResolvedValue({ installed: true }),
    marketUninstall: vi.fn().mockResolvedValue({ uninstalled: true }),
  } as unknown as TenonApi;
}

const t = (key: string) => key;

function renderPanel(api: TenonApi, props: Partial<Parameters<typeof SkillsSettings>[0]> = {}) {
  return render(
    <SkillsSettings
      api={api}
      t={t}
      projects={[{ id: "p1", label: "Tenon" }]}
      disabled={[]}
      onDisabledChange={() => {}}
      onSaved={() => {}}
      {...props}
    />
  );
}

describe("SkillsSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("confirm", vi.fn().mockReturnValue(true));
  });
  afterEach(() => vi.unstubAllGlobals());

  it("renders merged list with scope badges and enabled state", async () => {
    renderPanel(makeApi());
    await waitFor(() => expect(screen.getByText("提交助手")).toBeTruthy());
    expect(screen.getByText("api-mig")).toBeTruthy();
    expect(screen.getByTestId("skills-toggle-commit-helper")).toBeTruthy();
    expect((screen.getByTestId("skills-toggle-commit-helper") as HTMLInputElement).checked).toBe(
      true
    );
    expect((screen.getByTestId("skills-toggle-api-mig") as HTMLInputElement).checked).toBe(false);
  });

  it("toggles via PUT /settings skills.disabled (immediate, merged view back)", async () => {
    const api = makeApi();
    const onDisabledChange = vi.fn();
    const onSaved = vi.fn();
    renderPanel(api, { onDisabledChange, onSaved });
    await waitFor(() => expect(screen.getByTestId("skills-toggle-commit-helper")).toBeTruthy());

    fireEvent.click(screen.getByTestId("skills-toggle-commit-helper"));
    await waitFor(() =>
      expect(api.putSettings).toHaveBeenCalledWith({
        skills: { disabled: ["commit-helper"] },
      })
    );
    expect(onDisabledChange).toHaveBeenCalledWith(["commit-helper"]);
    expect(onSaved).toBeTruthy();
    await waitFor(() =>
      expect(
        (screen.getByTestId("skills-toggle-commit-helper") as HTMLInputElement).checked
      ).toBe(false)
    );
  });

  it("opens skill source, edits and saves through global endpoint", async () => {
    const api = makeApi();
    renderPanel(api);
    await waitFor(() => expect(screen.getByTestId("skills-open-commit-helper")).toBeTruthy());

    fireEvent.click(screen.getByTestId("skills-open-commit-helper"));
    await waitFor(() => expect(screen.getByTestId("skills-content")).toBeTruthy());
    expect((screen.getByTestId("skills-content") as HTMLTextAreaElement).value).toContain(
      "提交助手"
    );

    fireEvent.change(screen.getByTestId("skills-content"), {
      target: { value: "---\nname: 提交助手\n---\n新正文" },
    });
    fireEvent.click(screen.getByTestId("skills-save"));
    await waitFor(() => expect(api.updateSkill).toHaveBeenCalledWith("commit-helper", expect.stringContaining("新正文")));
  });

  it("creates a new global skill and deletes after confirm", async () => {
    const api = makeApi();
    renderPanel(api);
    await waitFor(() => expect(screen.getByTestId("skills-new-name")).toBeTruthy());

    fireEvent.change(screen.getByTestId("skills-new-name"), {
      target: { value: "../evil" },
    });
    fireEvent.click(screen.getByTestId("skills-new"));
    expect(screen.getByRole("alert").textContent).toContain("invalid_name");
    expect(api.createSkill).not.toHaveBeenCalled();

    fireEvent.change(screen.getByTestId("skills-new-name"), {
      target: { value: "note-writer" },
    });
    fireEvent.click(screen.getByTestId("skills-new"));
    await waitFor(() =>
      expect(api.createSkill).toHaveBeenCalledWith("note-writer", expect.stringContaining("---"))
    );

    fireEvent.click(screen.getByTestId("skills-delete-commit-helper"));
    await waitFor(() => expect(api.deleteSkill).toHaveBeenCalledWith("commit-helper"));
  });

  it("project scope routes save and delete through project file APIs", async () => {
    const api = makeApi();
    renderPanel(api);
    await waitFor(() => expect(screen.getByTestId("skills-scope")).toBeTruthy());

    fireEvent.change(screen.getByTestId("skills-scope"), { target: { value: "p1" } });
    await waitFor(() => expect(api.listSkills).toHaveBeenCalledWith("p1"));
    await waitFor(() => expect(screen.getByTestId("skills-open-api-mig")).toBeTruthy());

    fireEvent.click(screen.getByTestId("skills-open-api-mig"));
    await waitFor(() => expect(screen.getByTestId("skills-content")).toBeTruthy());
    fireEvent.change(screen.getByTestId("skills-content"), {
      target: { value: "项目技能新正文" },
    });
    fireEvent.click(screen.getByTestId("skills-save"));
    await waitFor(() =>
      expect(api.writeFile).toHaveBeenCalledWith("p1", ".tenon/skills/api-mig/SKILL.md", "项目技能新正文")
    );

    fireEvent.click(screen.getByTestId("skills-delete-api-mig"));
    await waitFor(() =>
      expect(api.fileOps).toHaveBeenCalledWith("p1", [
        { op: "delete", path: ".tenon/skills/api-mig/SKILL.md" },
      ])
    );
  });
});
