// 真 daemon 侧栏源码区 E2E（§7.2 v1.139）：「源码」上下拆分侧栏——上区任务流恒在、
// 下区 active 项目文件树，线程主区不动；点文件弹编辑器浮层（✕ 收回）；✕ 收源码区。
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

test("sidebar source pane splits the sidebar and pops the editor", async ({ page }) => {
  test.setTimeout(90_000);
  page.on("dialog", (dialog) => void dialog.accept());
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  // 默认：整栏任务流，无源码区。
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await expect(page.getByTestId("pe-source-pane")).toHaveCount(0);

  // active 项目行「源码」钮：侧栏上下拆分——下区文件树出现，线程主区不动。
  const modeToggle = page.locator(".pe-group-row.active .pe-view-toggle");
  await modeToggle.click();
  await expect(page.getByTestId("pe-source-pane")).toBeVisible();
  await expect(page.getByTestId("file-tree")).toBeVisible();
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await expect(page.locator(".zone-thread")).toBeVisible();

  // 单击文件：编辑器浮层弹出（v1.110 形态），✕ 收回。
  // 锚定正则：multiproject 同跑会留下 e2e-a.txt，子串匹配会撞。
  await page.getByTestId("file-tree").getByText("a.txt", { exact: true }).click();
  await expect(page.getByTestId("editor-overlay")).toBeVisible();
  await expect(page.getByRole("tab", { name: /^a\.txt/ })).toBeVisible();
  await page.getByTestId("editor-overlay-close").click();
  await expect(page.getByTestId("editor-overlay")).toHaveCount(0);
  await expect(page.getByTestId("pe-source-pane")).toBeVisible();

  // 源码区头部 ✕ 收起：侧栏回整栏任务流；刷新不自动开（会话级状态）。
  await page.getByTestId("pe-source-close").click();
  await expect(page.getByTestId("pe-source-pane")).toHaveCount(0);
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await page.reload();
  await expect(page.getByTestId("task-input-box")).toBeVisible();
  await expect(page.getByTestId("pe-source-pane")).toHaveCount(0);
});
