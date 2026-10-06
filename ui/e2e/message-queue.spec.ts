// 真 daemon 发送消息队列 E2E（§9.1 v1.147）：运行态入队 / 回合自然完成自动续跑 /
// ✕ 移除 / 停止冻结队列 / 冻结期手动续发。慢窗口由 mock-model-server 的
// E2E_SLOW_TURN_MS=<n> 指令提供（首趟模型调用延迟），断言时序确定。
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

/** 每个用例独立新会话：页面刷新后 App 自动激活最近会话（非草稿态），
 * 队列 id 按会话单调递增——不点「+ New task」会复用上一用例会话（实测 q1 断言落空）。 */
async function openFreshTask(page: import("@playwright/test").Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true }),
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();
}

test("运行态发送自动入队，回合自然完成后自动续跑", async ({ page }) => {
  test.setTimeout(120_000);
  await openFreshTask(page);
  // 首条慢任务：首趟模型调用延迟 6s，撑出确定的运行窗口
  await page.getByTestId("task-input").fill("E2E_SLOW_TURN_MS=6000 慢任务");
  await page.getByTestId("send").click();
  // 运行态双钮并列：停止出现、发送保留入队语义
  await expect(page.getByTestId("stop")).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId("send")).toBeEnabled();

  // 运行态发送 → 排队气泡 + 「已排队」徽标 + spinner 行计数
  await page.getByTestId("task-input").fill("排队消息一");
  await page.getByTestId("send").click();
  await expect(page.getByTestId("queue-item-q1")).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId("queue-badge")).toHaveText("Queued");
  await expect(page.getByTestId("queue-count")).toHaveText("Queue 1");

  // 再入队一条 → Queue 2；✕ 移除第一条 → Queue 1（被移除者不得进线程）
  await page.getByTestId("task-input").fill("排队消息二");
  await page.getByTestId("send").click();
  await expect(page.getByTestId("queue-item-q2")).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId("queue-count")).toHaveText("Queue 2");
  await page.getByTestId("queue-remove-q1").click();
  await expect(page.getByTestId("queue-item-q1")).toHaveCount(0, { timeout: 10_000 });
  await expect(page.getByTestId("queue-count")).toHaveText("Queue 1");

  // 慢回合自然完成 → daemon 自动出队「排队消息二」续跑（.turn-task 用户回合气泡，
  // 区别于排队气泡的 .queue-text）→ 队列清空、运行态收尾
  await expect(page.locator(".turn-task", { hasText: "排队消息二" })).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByTestId("stop")).toHaveCount(0, { timeout: 30_000 });
  await expect(page.getByTestId("queue-list")).toHaveCount(0, { timeout: 15_000 });
});

test("停止后队列冻结保留，冻结期手动续发", async ({ page }) => {
  test.setTimeout(120_000);
  await openFreshTask(page);
  // 写任务 + 慢标记：首趟延迟后返回 apply_patch 工具调用，停止在工具检查点生效
  const b64 = Buffer.from("written via queue e2e\n").toString("base64");
  const task = `E2E_WRITE\ne2e-queue.txt\n${b64} E2E_SLOW_TURN_MS=6000`;
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();
  await expect(page.getByTestId("stop")).toBeVisible({ timeout: 10_000 });

  // 运行窗口内入队
  await page.getByTestId("task-input").fill("排队消息三");
  await page.getByTestId("send").click();
  await expect(page.getByTestId("queue-item-q1")).toBeVisible({ timeout: 10_000 });

  // 停止 → paused：队列冻结不清空，条目出现手动发送钮
  await page.getByTestId("stop").click();
  await expect(page.getByTestId("resume")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("queue-item-q1")).toBeVisible();
  await expect(page.getByTestId("queue-send-q1")).toBeVisible();

  // 手动续发（先出队后投递，防 drain 重复）：新回合即时完成，队列清空、状态回 done
  await page.getByTestId("queue-send-q1").click();
  await expect(page.getByTestId("queue-item-q1")).toHaveCount(0, { timeout: 30_000 });
  await expect(page.getByTestId("stop")).toHaveCount(0, { timeout: 30_000 });
  await expect(page.getByTestId("resume")).toHaveCount(0);
});
