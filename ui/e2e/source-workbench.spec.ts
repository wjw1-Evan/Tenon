// 真 daemon 源码工作台 E2E（§7.2 v1.137）：「源码」整体跳转——主区切工作台（文件树 +
// 内嵌编辑器），线程隐藏挂载；刷新后 per-project 模式记忆恢复；「任务」切回线程。
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

test("source mode jumps the whole main area to the workbench and back", async ({ page }) => {
  test.setTimeout(90_000);
  page.on("dialog", (dialog) => void dialog.accept());
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  // 默认任务模式：线程主区可见，无工作台。
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await expect(page.getByTestId("source-workbench")).toHaveCount(0);

  // active 项目行「源码」钮：主区整体跳转——工作台（文件树 + 内嵌编辑器）替代线程；
  // 线程隐藏挂载（display:none）而非卸载。
  const modeToggle = page.locator(".pe-group-row.active .pe-view-toggle");
  await modeToggle.click();
  await expect(page.getByTestId("source-workbench")).toBeVisible();
  await expect(page.getByTestId("file-tree")).toBeVisible();
  await expect(page.locator(".zone-thread")).toBeHidden();

  // 单击文件：落工作台内嵌编辑器（tab 出现），不弹浮层。
  // exact：同跑套件里 multiproject 会在该项目留下 e2e-a.txt，子串匹配会撞。
  await page.getByTestId("file-tree").getByText("a.txt", { exact: true }).click();
  await expect(page.getByTestId("editor-pane")).toBeVisible();
  await expect(page.getByRole("tab", { name: "a.txt" })).toBeVisible();
  await expect(page.getByTestId("editor-overlay")).toHaveCount(0);

  // 刷新：per-project 模式记忆（tenon:peView）恢复，启动即源码模式。
  await page.reload();
  await expect(page.getByTestId("source-workbench")).toBeVisible();
  await expect(page.getByTestId("file-tree")).toBeVisible();

  // 「任务」切回：工作台退场，线程复原。
  await modeToggle.click();
  await expect(page.getByTestId("source-workbench")).toHaveCount(0);
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await expect(page.locator(".zone-thread")).toBeVisible();
});
