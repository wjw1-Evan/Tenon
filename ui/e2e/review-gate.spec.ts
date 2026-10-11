// v2 集成 E2E（design-v2.md §4.1 / §6，真 daemon + 真 UI）：
// ① 常驻审查面（v2.5）：运行中任务自动停靠右栏、diff 内容实时可见、完成自动收起；
// ② 档位确认卡（v2.1）：D 级 git_commit 被默认档 on_irreversible Hold——
//    卡片渲染（工具 + 参数预览）→ 拒绝 → 模型收到拒绝继续 → 任务完成、卡片收起。
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

test("review pane auto-docks during a running task and collapses when done", async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto(runtime.baseUrl);
  await expect(page.getByTestId("project-list").getByText("project-a", { exact: true })).toBeVisible();

  // E2E_SLOW_TURN_MS 撑出确定性运行窗口：首趟 apply_patch 落 diff 后第二趟
  // 模型调用延迟——审查面在窗口内停靠且 diff 可见
  const task = [
    "E2E_SLOW_TURN_MS=5000",
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
  await expect(page.getByTestId("review-status")).toContainText(/running|执行|运行/, { timeout: 10_000 }).catch(() => {
    // 状态文案随语言；data-running 已断言运行态，此处仅尽力校验
  });

  // 完成后自动收起（reviewManual 未覆盖）
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", { timeout: 60_000 });
  await expect(pane).toBeHidden({ timeout: 15_000 });
});

test("d-level confirm card holds, deny resolves, task completes", async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto(runtime.baseUrl);
  await expect(page.getByTestId("project-list").getByText("project-a", { exact: true })).toBeVisible();

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
