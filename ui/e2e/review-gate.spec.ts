// v2 集成 E2E（design-v2.md §6，真 daemon + 真 UI）：
// ① 常驻审查面（v2.5）：运行中任务自动停靠右栏、diff 内容实时可见、完成自动收起；
// ② diff 级反馈回路（v2.7）：运行中审查面选中改动行 → 反馈卡发送（运行态入队）→
//    回合完成队列消费 → 第二回合按反馈改码落盘——「边跑边审、审完即改」闭环；
// ③ 审查摘要卡（v2.8）：运行中审查面呈现子任务进度与改动产物（mock E2E_SUBTASKS
//    驱动；证据卡在回合尾验证阶段产生、面板随完成收起，由 vitest App 级覆盖）；
// ④ 运行仪表（v2.9）：daemon 每轮模型调用自动发 context_usage（零 mock 改动）——
//    审查面占比条 + 图例 + 累计成本行真链路可见。
// （v2.1 档位确认卡场景随 v2.37 档位退役移除——确认卡与安全徽标 UI 已不存在。）
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
 *  apply_patch），审查面断言全部落空。 */
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

  // v2.51 回归：diff 正文容器内容自适应高——spacer 若回退 absolute 定位，
  // 父级 auto 高塌缩为 padding+border=18px 单行（审查面 / 时间线 diff 同病）
  const diffBody = pane.getByTestId("diff-body");
  await expect
    .poll(async () => (await diffBody.boundingBox())?.height ?? 0, { timeout: 5_000 })
    .toBeGreaterThanOrEqual(4 * 18);

  // v2.11 宽度拖拽记忆：左缘把手左拖 60px → 审查面增宽（boundingBox 实测）
  const before = (await pane.boundingBox())!.width;
  const hb = (await page.getByTestId("resize-review").boundingBox())!;
  await page.mouse.move(hb.x + hb.width / 2, hb.y + 80);
  await page.mouse.down();
  await page.mouse.move(hb.x + hb.width / 2 - 60, hb.y + 80, { steps: 2 });
  await page.mouse.up();
  const after = (await pane.boundingBox())!.width;
  expect(after - before).toBeGreaterThanOrEqual(30);

  // 完成后自动收起（reviewManual 未覆盖）
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", { timeout: 60_000 });
  await expect(pane).toBeHidden({ timeout: 15_000 });
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

test("review digest cards surface subtask progress and changed files during a run", async ({ page }) => {
  test.setTimeout(90_000);
  await openFreshTask(page);

  const file = "e2e-digest.txt";
  const task = [
    "E2E_SLOW_ALL_MS=4000",
    "E2E_SUBTASKS",
    JSON.stringify([
      { title: "write the digest fixture", status: "done" },
      { title: "verify the digest fixture", status: "in_progress" },
    ]),
    "E2E_WRITE",
    file,
    Buffer.from("digest payload\n").toString("base64"),
  ].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();

  // 审查面停靠 → 摘要区：子任务卡（清单 + n/m）与产物卡（改动文件）实时可见
  const pane = page.getByTestId("review-pane");
  await expect(pane).toBeVisible({ timeout: 30_000 });
  // v2.38 工具面板 tab 化：摘要五卡 + 运行仪表由 diff 页迁入「概览」页（默认
  // 仍为 diff 页）——切概览页后断言（与 reviewToolPanel vitest 同法）
  await page.getByTestId("review-tab-overview").click();
  const subtasks = page.getByTestId("review-subtasks");
  await expect(subtasks).toBeVisible({ timeout: 30_000 });
  await expect(subtasks).toContainText("write the digest fixture");
  await expect(subtasks).toContainText("1/2");
  const artifacts = page.getByTestId("review-artifacts");
  await expect(artifacts).toBeVisible({ timeout: 30_000 });
  await expect(artifacts).toContainText(file);

  // v2.9 运行仪表：daemon 每轮模型调用后自动发 context_usage（真链路零 mock
  // 改动）——五段占比条 + 图例（含 tok 数）+ 累计成本行
  const gauges = page.getByTestId("review-gauges");
  await expect(gauges).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("review-ctx-bar").locator(".review-ctx-seg")).toHaveCount(5);
  await expect(page.getByTestId("review-ctx-legend")).toContainText("System prompt");
  await expect(page.getByTestId("review-ctx-cost")).toContainText(/\$/);

  // 完成收起（摘要区随面板）
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", { timeout: 60_000 });
  await expect(pane).toBeHidden({ timeout: 15_000 });
});
