// 对话回合操作 E2E（真 daemon + 真 UI）：从线程气泡下功能钮驱动的两条链路：
// ① 编辑重发（§7.5 v2.58 三件套③）：已结束回合「编辑」→ 任务文本回填输入框 →
//    改写后发送即新回合；不截断历史（原回合仍在）。
// ② 审查发现一键修复（§7.5 v2.66）：任务文本恰为 en 语言包审查模板的回合，其
//    终答下渲染「修复以上发现」→ 点击注入自包含修复任务（模板 + 审查回复全文）。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";

interface Runtime {
  baseUrl: string;
  projectA: string;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json",
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

async function openFreshTask(page: Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true }),
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();
}

test("edit a finished turn refills the input and resends as a new turn", async ({ page }) => {
  test.setTimeout(120_000);
  await openFreshTask(page);

  const firstTask = "E2E_REPLY\nE2E first answer";
  await page.getByTestId("task-input").fill(firstTask);
  await page.getByTestId("send").click();
  const feed = page.getByTestId("agent-feed");
  await expect(feed).toContainText("E2E first answer", { timeout: 60_000 });
  await expect(page.getByTestId("stop")).toHaveCount(0, { timeout: 30_000 });

  // 编辑钮：任务文本逐字回填输入框
  await page.locator('[data-testid^="turn-edit-"]').first().click();
  await expect(page.getByTestId("task-input")).toHaveValue(firstTask);

  // 改写后发送 → 新回合；原回合不截断（两条用户气泡并存）
  await page.getByTestId("task-input").fill("E2E_REPLY\nE2E edited answer");
  await page.getByTestId("send").click();
  await expect(feed).toContainText("E2E edited answer", { timeout: 60_000 });
  await expect(page.locator(".turn-task", { hasText: "E2E first answer" })).toHaveCount(1);
  await expect(page.locator(".turn-task", { hasText: "E2E edited answer" })).toHaveCount(1);
});

test("review turn offers fix-findings and injects a self-contained fix task", async ({ page }) => {
  test.setTimeout(120_000);
  // E2E locale = en-US：修复钮按「任务文本恰为审查模板」识别，读 en 语言包取模板
  const en = JSON.parse(
    await readFile(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../src/locales/en.json"), "utf8"),
  ) as Record<string, string>;
  const reviewTask = en["slash.review_task"];
  const fixTemplate = en["slash.fix_findings_task"];

  await openFreshTask(page);
  await page.getByTestId("task-input").fill(reviewTask);
  await page.getByTestId("send").click();
  const feed = page.getByTestId("agent-feed");
  // mock 对无指令任务回固定文本（视为审查回复/终答）
  await expect(feed).toContainText("E2E task missing write directive", { timeout: 60_000 });

  const fixBtn = page.locator('[data-testid^="turn-fix-"]');
  await expect(fixBtn).toHaveCount(1, { timeout: 15_000 });
  await fixBtn.click();

  // 新回合任务 = 修复模板 + 空行 + 审查回复全文（自包含，不依赖携带存活）
  const newTurn = page.locator(".turn-task").last();
  await expect(newTurn).toContainText(fixTemplate.slice(0, 40), { timeout: 30_000 });
  await expect(newTurn).toContainText("E2E task missing write directive");
});
