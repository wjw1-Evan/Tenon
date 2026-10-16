// 检查点回滚动线 E2E（§10.3，真 daemon + 真快照库）：代理写入落盘 → 时间线
// 出现检查点 → 回滚文件恢复原状 →「撤销回滚」（unrevert）找回 → 再次回滚收尾。
// 此前回滚 / unrevert 仅 daemon 集成测试与 vitest（mock api）覆盖，shadow git
// 写写路径与时间线 UI 的浏览器级串联无回归。
import { readFile, stat } from "node:fs/promises";
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

/** 全新会话入口（review-gate spec 同款纪律）：共享 daemon 下不点会复用上一
 *  spec 留下的会话，检查点混入即断言失真。 */
async function openFreshTask(page: import("@playwright/test").Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();
}

async function fileExists(p: string): Promise<boolean> {
  try {
    await stat(p);
    return true;
  } catch {
    return false;
  }
}

test("agent write creates a checkpoint, rollback reverts it, unrevert restores it", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await openFreshTask(page);

  // 代理真实落盘（新建文件——回滚即删除，断言「恢复原状」无歧义）
  const file = "e2e-rollback.txt";
  const target = path.join(runtime.projectA, file);
  const task = [
    "E2E_WRITE",
    file,
    Buffer.from("revert me\n").toString("base64"),
  ].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();
  await expect
    .poll(async () => (await readFile(target, "utf8").catch(() => "")), { timeout: 60_000 })
    .toContain("revert me");
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", {
    timeout: 60_000,
  });

  // 展开底部面板并回到时间线 tab（timeline 钮无 testid，为 tabs 首钮；底栏
  // 开合与 tab 选择经项目 ui-state 持久化——上一 spec 留下的展开态须兜底）。
  // 检查点行 1.5s 轮询出现。单次写入产生多行检查点（会话建卡 / B 级写前快照 /
  // 回合收尾），列表 newest-first——写前树是最旧一行（末行），回滚到它即删除
  // 新建文件。
  const collapsedToggle = page.getByTestId("bottom-open");
  if (await collapsedToggle.count()) await collapsedToggle.click();
  await expect(page.getByTestId("bottom-close")).toBeVisible();
  const timeline = page.getByTestId("timeline");
  if (!(await timeline.count())) {
    await page.locator(".bottom-tabs button").first().click();
  }
  await expect(timeline).toBeVisible();
  const node = timeline.locator("li.timeline-node").last();
  await expect(node).toContainText(file, { timeout: 15_000 });

  // 回滚：文件自盘上消失（回到写入前树）
  await node.getByRole("button", { name: "Roll back to this node" }).click();
  await expect
    .poll(async () => fileExists(target), { timeout: 30_000 })
    .toBe(false);

  // 撤销回滚（unrevert，§10.3）：文件找回且内容忠实
  await timeline.locator(".timeline-unrevert").click();
  await expect
    .poll(async () => fileExists(target), { timeout: 30_000 })
    .toBe(true);
  await expect(await readFile(target, "utf8")).toContain("revert me");

  // 再次回滚收尾：清理 fixture，防后续 spec 的查找 / 搜索断言被该文件干扰
  await node.getByRole("button", { name: "Roll back to this node" }).click();
  await expect
    .poll(async () => fileExists(target), { timeout: 30_000 })
    .toBe(false);
});
