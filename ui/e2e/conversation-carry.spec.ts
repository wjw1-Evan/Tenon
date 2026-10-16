// v2.72 跨任务对话携带 E2E（§9.1，真 daemon + 真 UI）：同一会话内第二个任务启动
// 时，daemon 从事件日志（v2.48 懒重建同样可用）重建最近对话轮次并前置进 messages
// ——用户文本与上一轮终答对模型可见，工具历史不携带。mock 回报 messages 实况
// （CARRY user/answer/tools），据此断言「事件日志 → 携带布局 → 模型请求」端到端生效。
//
// 判别纪律：任务文本亦经 L2 goals 进系统提示，故 mock 只统计 role=user 的历史
// 消息（排除本条任务）与 role=assistant 的终答，system 消息不计——避免把 goals
// 注入误判为携带。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

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

/** 全新会话入口（同 message-queue 纪律）：共享 daemon 下不点会复用上一 spec
 *  留下的会话，其工具历史使 mock 首趟直出文本，断言全部落空。 */
async function openFreshTask(page: import("@playwright/test").Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true }),
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();
}

test("second task in the same session carries prior conversation but not tool history", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await openFreshTask(page);

  const anchor = "E2E_CARRY_ANCHOR_7f3a";
  const firstAnswer = "E2E_CARRY_REPLY";
  // 轮 1：锚点（即被携带的用户文本）+ E2E_REPLY 固定终答——两者都应进轮 2 上下文
  await page.getByTestId("task-input").fill(`${anchor}\nE2E_REPLY\n${firstAnswer}`);
  await page.getByTestId("send").click();
  const feed = page.getByTestId("agent-feed");
  await expect(feed).toContainText(firstAnswer, { timeout: 60_000 });
  await expect(page.getByTestId("stop")).toHaveCount(0, { timeout: 30_000 });

  // 轮 2：同一会话内追问（不点「+ New task」）——daemon 应前置轮 1 的对话
  await page.getByTestId("task-input").fill(`E2E_CARRY_CHECK\n${anchor}`);
  await page.getByTestId("send").click();
  await expect(feed).toContainText("CARRY user=yes answer=yes tools=0", { timeout: 60_000 });
});
