// v2 集成 E2E（design-v2.md §4.1 / §6，真 daemon + 真 UI）：
// ① 常驻审查面（v2.5）：运行中任务自动停靠右栏、diff 内容实时可见、完成自动收起；
// ② 档位确认卡（v2.1）：D 级 git_commit 被默认档 on_irreversible Hold——
//    卡片渲染（工具 + 参数预览）→ 拒绝 → 模型收到拒绝继续 → 任务完成、卡片收起；
// ③ diff 级反馈回路（v2.7）：运行中审查面选中改动行 → 反馈卡发送（运行态入队）→
//    回合完成队列消费 → 第二回合按反馈改码落盘——「边跑边审、审完即改」闭环。
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

/** 全新会话入口（message-queue spec 同款纪律）：共享 daemon 下不点会复用
 *  上一 spec 留下的会话——其工具历史使 mock server 首趟直出文本（无
 *  apply_patch / git_commit），档位与审查面断言全部落空。 */
async function openFreshTask(page: import("@playwright/test").Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true }),
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();
}

test("review pane auto-docks during a running task and collapses when done", async ({ page }) => {
  test.setTimeout(90_000);
  await openFreshTask(page);

  // E2E_SLOW_ALL_MS：apply_patch 落 diff 后第二趟模型调用延迟 4s——撑出
  // 确定性运行窗口供审查面内容断言（首趟无延迟，diff 尽早到达）
  const task = [
    "E2E_SLOW_ALL_MS=4000",
    "E2E_WRITE",
    "e2e-review.txt",
    Buffer.from("review pane payload\n").toString("base64"),
  ].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();

  const pane = page.getByTestId("review-pane");
  await expect(pane).toBeVisible({ timeout: 30_000 });
  await expect(pane).toHaveAttribute("data-running", "1");
  await expect(pane).toContainText("e2e-review.txt", { timeout: 20_000 });

  // 完成后自动收起（reviewManual 未覆盖）
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", { timeout: 60_000 });
  await expect(pane).toBeHidden({ timeout: 15_000 });
});

test("d-level confirm card holds, deny resolves, task completes", async ({ page }) => {
  test.setTimeout(90_000);
  await openFreshTask(page);

  // git_commit 是 D 级：默认档 on_irreversible → Hold 等待确认（v2.1）
  const task = ["E2E_GIT_COMMIT", "e2e should not land"].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();

  const card = page.getByTestId("confirm-card");
  await expect(card).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("confirm-tool")).toHaveText("git_commit");
  await expect(card).toContainText("e2e should not land");
  await expect(page.getByTestId("confirm-level")).toHaveText("D");

  // 拒绝：模型收到拒绝结果继续回合 → 完成、卡片收起
  await page.getByTestId("confirm-deny").click();
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", { timeout: 30_000 });
  await expect(card).toBeHidden({ timeout: 15_000 });
});

test("diff line feedback loop: review while running, feedback re-edits the file", async ({ page }) => {
  test.setTimeout(120_000);
  await openFreshTask(page);

  const file = "e2e-feedback.txt";
  const v2 = "first payload v2";
  const v3 = "first payload v3 by feedback";
  // 6s 运行窗口（apply_patch 落 diff 后第二趟延迟）供选中 + 反馈发送
  const task = [
    "E2E_SLOW_ALL_MS=6000",
    "E2E_WRITE",
    file,
    Buffer.from(`${v2}\n`).toString("base64"),
  ].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();

  const pane = page.getByTestId("review-pane");
  await expect(pane).toBeVisible({ timeout: 30_000 });
  await expect(pane).toContainText(v2, { timeout: 20_000 });

  // 选中 + 改动行 → 反馈卡（上下文标签指向该文件）
  await page.getByTestId("diff-line").filter({ hasText: v2 }).first().click();
  const card = page.getByTestId("review-feedback");
  await expect(card).toBeVisible();
  await expect(page.getByTestId("review-fb-target")).toContainText(file);
  // 反馈指令内嵌第二次写指令：mock server 以最后一条 user 消息驱动第二回合
  await page.getByTestId("review-fb-input").fill(
    ["E2E_WRITE", file, Buffer.from(`${v3}\n`).toString("base64")].join("\n")
  );
  await page.getByTestId("review-fb-send").click();
  await expect(card).toBeHidden();

  // 运行态发送入队（v1.147）→ 回合完成消费 → 第二回合改码落盘（闭环；
  // mock 的 apply_patch range:null 为追加语义，断言以包含 v3 为准）
  const target = path.join(runtime.projectA, file);
  await expect
    .poll(async () => (await readFile(target, "utf8").catch(() => "")), { timeout: 90_000 })
    .toContain(v3);
});
