// 程序开发工具面 E2E（真 daemon + 真 UI）：mock 发工具调用、daemon 真执行、
// UI 步骤卡呈现真实输出——覆盖两类此前无 E2E 的开发向工具链：
// ① 后台任务三件套（§9.2 v2.64/v2.70）：run_command background=true 启真子进程
//    → task_output 列表模式枚举（id + 状态 + 命令摘要）→ task_kill 终止；
// ② PTC 程序化工具调用（§9.2 v2.63）：run_code 单回合内嵌 JS 调内层 tool()
//    读文件并 log——一次回合完成批量调用。
// 步骤卡默认折叠（仅失败默认展开），故输出断言前先点开标题行。
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

/** 展开指定标签的步骤卡并返回其定位器（输出只在展开后可见）。 */
async function expandStep(page: Page, label: string) {
  const step = page.getByTestId("turn-step").filter({ hasText: label });
  await expect(step).toHaveCount(1);
  await step.getByRole("button").first().click();
  return step;
}

test("background command: launch, list via task_output and kill via task_kill", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await openFreshTask(page);
  await page.getByTestId("task-input").fill("E2E_BG");
  await page.getByTestId("send").click();

  const feed = page.getByTestId("agent-feed");
  await expect(feed).toContainText("E2E background task flow complete", { timeout: 90_000 });

  // 步骤 1：run_command background=true —— 真子进程启动，返回任务号
  const launch = await expandStep(page, "Run command");
  await expect(launch).toContainText("后台任务 #1 已启动");
  await expect(launch).toContainText("sleep 30");

  // 步骤 2：task_output 省略 id = 列表模式（v2.70）——一行摘要含任务号 + 运行态
  const list = await expandStep(page, "Task output");
  await expect(list).toContainText("后台任务 1 项");
  await expect(list).toContainText("#1 [运行中] sleep 30");

  // 步骤 3：task_kill —— 终止信号已发出
  const kill = await expandStep(page, "Kill task");
  await expect(kill).toContainText("后台任务 #1 已发送终止信号");
});

test("run_code PTC batch calls nested tools in a single round", async ({ page }) => {
  test.setTimeout(120_000);
  await openFreshTask(page);
  await page.getByTestId("task-input").fill("E2E_RUNCODE");
  await page.getByTestId("send").click();

  await expect(page.getByTestId("agent-feed")).toContainText("E2E PTC flow complete", {
    timeout: 90_000,
  });

  // 单张 run_code 步骤卡：内层 tool("read_file","a.txt") 被真实执行——
  // console 输出含项目文件内容，返回值附调用计数
  const step = await expandStep(page, "PTC batch");
  await expect(step).toContainText("PTC alpha"); // console.log("PTC " + 文件正文)
  await expect(step).toContainText("tool 调用 1 次");
});
