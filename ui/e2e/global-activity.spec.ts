// v2.12 全局活动视图 E2E（design-v2.md §6「命令面板 + 侧栏入口的跨项目任务总览」，
// 真 daemon + 真 UI）：rail activity 钮与命令面板双入口打开跨项目任务总览浮层，
// 任务行点击跳转并关闭——v1.98 克制不变式（非常驻、按需浮层）。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

interface Runtime {
  baseUrl: string;
  daemonPid?: number;
  modelPid?: number;
  projectA: string;
  projectB: string;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../test-results/tenon-runtime.json");
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

test("global activity opens from rail and command palette, row click navigates", async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true }),
  ).toBeVisible();

  // 全新会话造一条任务（fresh daemon 下保证 project-a 有可总览的任务行）
  await page.getByTestId("project-new-task").click();
  const task = [
    "E2E_WRITE",
    "e2e-activity.txt",
    Buffer.from("activity fixture\n").toString("base64"),
  ].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", { timeout: 60_000 });

  // 入口①：rail activity 钮 → 浮层（project-a 分组 + 任务行可见）
  await page.getByTestId("rail-activity").click();
  const dialog = page.getByTestId("global-activity");
  await expect(dialog).toBeVisible();
  const groups = dialog.getByTestId("activity-group");
  await expect(groups.first()).toBeVisible({ timeout: 15_000 });
  expect(await groups.count()).toBeGreaterThanOrEqual(1);
  await expect(dialog.getByTestId("activity-row").first()).toContainText("E2E");
  // Esc 关闭（克制：按需浮层）
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();

  // 入口②：命令面板「Global activity」→ 浮层
  await page.keyboard.press("Meta+Shift+p");
  const palette = page.getByTestId("command-palette");
  await expect(palette).toBeVisible();
  await palette.locator("input").fill("Global activity");
  await palette.locator("input").press("Enter");
  await expect(dialog).toBeVisible();

  // 行点击 = 激活该会话并关闭浮层
  const row = dialog.getByTestId("activity-row").first();
  await row.click();
  await expect(dialog).toBeHidden();
  // 激活后会话线程可见（跳转闭环）
  await expect(page.getByTestId("agent-feed").first()).toBeVisible();
});
