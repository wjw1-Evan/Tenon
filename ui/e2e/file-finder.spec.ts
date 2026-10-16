// 模糊文件查找动线 E2E（§7.4 / §8.2）：Cmd/Ctrl+P 打开 FileFinder → 键入过滤
// → Enter 打开编辑器浮层；无命中空态 + Esc 关闭。此前该动线仅 vitest（jsdom
// mock fetch）覆盖，真 daemon /files/fuzzy（gitignore-aware + 评分排序）与
// v2.83 懒加载编辑器 chunk 的串联无浏览器级回归。
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

test("Cmd/Ctrl+P finds a file and Enter opens it in the editor overlay", async ({ page }) => {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  // 打开查找浮层，键入即查（120ms 防抖）：a.txt 为 fixture 根文件，最优评分居首
  const mod = process.platform === "darwin" ? "Meta" : "Control";
  await page.keyboard.press(`${mod}+P`);
  const finder = page.getByTestId("file-finder");
  await expect(finder).toBeVisible();
  const input = page.getByTestId("file-finder-input");
  await expect(input).toBeFocused();
  await input.fill("a.txt");
  await expect(finder.locator(".finder-path").first()).toHaveText("a.txt");

  // Enter 打开：浮层收起、编辑器浮层弹出、a.txt 标签可见（懒加载 chunk 解析后）
  await input.press("Enter");
  await expect(page.getByTestId("file-finder")).toHaveCount(0);
  await expect(page.getByTestId("editor-overlay")).toBeVisible();
  await expect(page.getByRole("tab", { name: /^a\.txt/ })).toBeVisible();
  // 真编辑器内容（monaco DOM 渲染）：fixture 原文可见
  await expect(page.locator(".editor-overlay .view-lines")).toContainText("alpha");

  // 无命中空态 + Esc 关闭：不弹编辑器
  await page.keyboard.press(`${mod}+P`);
  await expect(page.getByTestId("file-finder")).toBeVisible();
  await page.getByTestId("file-finder-input").fill("zzz-no-such-file");
  await expect(page.getByTestId("file-finder")).toContainText("No matching files");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("file-finder")).toHaveCount(0);
  await expect(page.getByTestId("editor-overlay")).toBeVisible(); // 既有浮层不受影响
});
