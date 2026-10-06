// 真 daemon 源码工作台 E2E（§7.2 v1.138）：弹出层宿主——「源码」弹出工作台（文件树 +
// 内嵌编辑器），线程主区不动（对话随时可用）；✕ 收回对话；刷新不自动弹（会话级状态）。
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

test("source workbench pops over the intact thread and hands back", async ({ page }) => {
  test.setTimeout(90_000);
  page.on("dialog", (dialog) => void dialog.accept());
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  // 默认任务对话：线程主区可见，无工作台。
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await expect(page.getByTestId("source-workbench")).toHaveCount(0);

  // active 项目行「源码」钮：工作台弹出（文件树 + 内嵌编辑器），线程主区不动。
  const modeToggle = page.locator(".pe-group-row.active .pe-view-toggle");
  await modeToggle.click();
  await expect(page.getByTestId("source-workbench")).toBeVisible();
  await expect(page.getByTestId("file-tree")).toBeVisible();
  await expect(page.getByTestId("task-input-box")).toBeVisible();

  // 单击文件：tab 落在工作台内嵌编辑器（无独立浮层）。
  // 锚定正则：multiproject 同跑会在项目 ui-state 留下 e2e-a.txt 标签，子串匹配会撞。
  await page.getByTestId("file-tree").getByText("a.txt", { exact: true }).click();
  await expect(page.getByTestId("editor-pane")).toBeVisible();
  await expect(page.getByRole("tab", { name: /^a\.txt/ })).toBeVisible();

  // ✕ 收回：弹出层退场，对话原样可见。
  await page.getByTestId("source-workbench-close").click();
  await expect(page.getByTestId("source-workbench")).toHaveCount(0);
  await expect(page.getByTestId("task-input-box")).toBeVisible();

  // 刷新：会话级状态不持久化（v1.110 弹出先例），不自动弹。
  await page.reload();
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await expect(page.getByTestId("source-workbench")).toHaveCount(0);
});
