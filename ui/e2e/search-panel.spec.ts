// 全局搜索动线 E2E（§7.4）：rail 搜索视图 → 正则查询 → 命中分组 → 点击命中
// 行打开编辑器。真 daemon /search（rg）链路此前仅 vitest mock 覆盖；含「编辑器
// 内容与命中行同屏」的浏览器级断言（monaco DOM 文本）。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

interface Runtime {
  baseUrl: string;
  projectA: string;
  projectB: string;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json"
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

test("search finds a fixture match and clicking it opens the editor at the hit", async ({
  page,
}) => {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  // rail 搜索视图：project-a 的 a.txt 含唯一 "alpha"（projectB 的 beta 不同项目不混入）
  await page.getByTestId("rail-search").click();
  const panel = page.getByTestId("search-panel");
  await expect(panel).toBeVisible();
  await panel.getByLabel("Search regex").fill("alpha");
  await panel.getByRole("button", { name: "Search", exact: true }).click();

  // 命中摘要 + 分组行：1 结果，组头为 a.txt，命中行 1:1
  await expect(panel).toContainText("1 results", { timeout: 15_000 });
  const group = panel.locator(".search-group", { hasText: "a.txt" });
  await expect(group).toHaveCount(1);
  const hit = group.locator(".search-hit").first();
  await expect(hit).toContainText("1:1");

  // 点击命中 → 编辑器浮层弹出，monaco 渲染命中文件内容
  await hit.click();
  await expect(page.getByTestId("editor-overlay")).toBeVisible();
  await expect(page.getByRole("tab", { name: /^a\.txt/ })).toBeVisible();
  await expect(page.locator(".editor-overlay .view-lines")).toContainText("alpha");
});
