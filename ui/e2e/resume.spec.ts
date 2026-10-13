// v2.49 持续运行 E2E（§9.1 / §7.5，真 daemon + 真 UI）：计划提交暂停 →
// 点「Resume」→ daemon 转续跑开新回合（本地化继续语入上下文）→ 新回合真实
// 写入文件 → 收尾完成。v2.49 起交互会话无回合上限（ADR-21），脚本可控的
// 暂停源改用计划模式（submit_plan → 终态 paused）。
// mock 相位脚本见 mock-model-server.mjs E2E_PLAN_RESUME（回合状态机分相）。
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
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json",
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

/** 全新会话入口（review-gate spec 同款纪律）：共享 daemon 下不点会复用上一
 *  spec 留下的会话——其历史里的 E2E_TOOL_LOOP 标记与工具结果会污染相位判定。 */
async function openFreshTask(page: import("@playwright/test").Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true }),
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();
}

test("plan pause shows the plan card and resume continues into a real write", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await openFreshTask(page);

  const file = "e2e-resume.txt";
  const payload = "resumed after plan pause\n";
  const task = ["E2E_PLAN_RESUME", file, Buffer.from(payload).toString("base64")].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();

  // 相 1 提交计划 → 计划暂停：计划卡可见（items 清单），「Resume」钮可见且可点
  await expect(page.getByTestId("turn-plan")).toBeVisible({ timeout: 60_000 });
  const resume = page.getByTestId("resume");
  await expect(resume).toBeVisible();
  await expect(resume).toBeEnabled();

  // 点恢复 → daemon 终态暂停转续跑：新 user 气泡（本地化继续语）可见，
  // 新回合真实落盘（相 2 apply_patch）后文本收尾（相 3）
  await resume.click();
  await expect(page.getByTestId("agent-feed")).toContainText("Continue", { timeout: 30_000 });
  const target = path.join(runtime.projectA, file);
  await expect
    .poll(async () => (await readFile(target, "utf8").catch(() => "")), { timeout: 60_000 })
    .toContain(payload);
  await expect(page.getByTestId("agent-feed")).toContainText(
    "E2E resume continuation complete",
    { timeout: 60_000 },
  );
});
