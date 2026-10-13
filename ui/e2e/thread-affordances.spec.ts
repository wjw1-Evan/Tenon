// v2.58 对话交互三件套 E2E（真 daemon + 真 UI）——jsdom 无布局引擎不可断言项：
// ① 输入框随内容自动增高：多行内容实测高度增长、清空回缩单行（min-height 兜底）；
// ② 回底浮钮：线程内容溢出后上滑离底（≥160px）浮钮显现（sticky 钉滚动口下缘），
//    点击回底（scrollTop 贴底）并消失。未读徽点为纯状态逻辑（离底 + 内容增长），
//    由 vitest agentThreadAffordances 覆盖，此处不断言时序。
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

test("input auto-grows with content and shrinks back when cleared", async ({ page }) => {
  await page.goto(runtime.baseUrl);
  const input = page.getByTestId("task-input");
  await expect(input).toBeVisible();
  // 空输入 = 单行基线（CSS min-height 46px 兜底）
  const baseline = (await input.boundingBox())!.height;
  // 12 行内容 → 实际增高（>2 倍基线；180px 封顶不触顶）
  await input.fill(Array.from({ length: 12 }, (_, i) => `第 ${i} 行`).join("\n"));
  const grown = (await input.boundingBox())!.height;
  expect(grown).toBeGreaterThan(baseline * 2);
  // 清空 → 回缩单行基线
  await input.fill("");
  const shrunk = (await input.boundingBox())!.height;
  expect(shrunk).toBeLessThan(grown - 20);
});

test("jump-to-bottom button appears when scrolled away and scrolls back on click", async ({
  page,
}) => {
  test.setTimeout(90_000);
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true }),
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();

  // 长任务文本（无指令 → mock 回复固定收尾语）以用户气泡全文渲染，撑出线程溢出
  const longTask = Array.from({ length: 120 }, (_, i) => `第 ${i} 行——撑出线程滚动`).join("\n");
  const input = page.getByTestId("task-input");
  await input.fill(longTask);
  await page.getByTestId("send").click();

  const feed = page.getByTestId("agent-feed");
  // 任务气泡可见 = 回合开启；线程内容溢出（scrollHeight > clientHeight）
  await expect(feed).toContainText("第 100 行", { timeout: 30_000 });
  await expect
    .poll(async () => feed.evaluate((el) => el.scrollHeight - el.clientHeight), { timeout: 10_000 })
    .toBeGreaterThan(160);

  // 上滑离底（程序化滚动同样触发 scroll 事件）→ 浮钮显现
  await feed.evaluate((el) => {
    el.scrollTop = 0;
  });
  const jump = page.getByTestId("agent-jump");
  await expect(jump).toBeVisible();

  // 点击回底：滚动口贴底、浮钮复位消失
  await jump.click();
  await expect
    .poll(async () => feed.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight))
    .toBeLessThan(160);
  await expect(jump).toBeHidden();
});
