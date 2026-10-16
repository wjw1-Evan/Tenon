// 受管 worktree 会话全环 E2E（§9.7 v1.87，设计 §18.1 测试矩阵列名场景）：草稿
// 工作区选「受管 worktree」→ 首发落库建会话（写边界 / 快照分片按 worktree 隔离）
// → 代理写入只落 worktree 副本、主根不可见 → 会话行出现「合并 / 丢弃」收尾钮 →
// 合并把文件三方并入主根、丢弃（window.confirm）后改动永不落主根。
// 此前该动线仅 daemon 集成测试与 vitest（mock api）覆盖，浏览器级串联无回归。
// 前置：global-setup 已把 project-a 落成 git 仓库（worktree add 需已有提交）。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type Locator, type Page } from "@playwright/test";

interface Runtime {
  baseUrl: string;
  projectA: string;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json"
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

/** 全新草稿并把工作区意图切到受管 worktree（会话随首条消息才落库）。 */
async function openManagedDraft(page: Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();
  // 不点项目行：active 项目默认展开（v1.63），点击只会收起、藏掉会话行
  await page.getByTestId("project-new-task").click();
  // 草稿上下文条（v1.126）：主根 / 受管 worktree 即「分支选择」的 Tenon 对应物
  await page.getByTestId("draft-workspace-select").selectOption("managed");
}

/** 发送写盘任务并等待回合完成（mock 管线真实落 worktree）。 */
async function runWriteTask(page: Page, file: string, content: string) {
  const task = ["E2E_WRITE", file, Buffer.from(content).toString("base64")].join("\n");
  await page.getByTestId("task-input").fill(task);
  await page.getByTestId("send").click();
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", {
    timeout: 90_000,
  });
}

/**
 * 等会话行出现（active 项目默认展开 v1.63；标题生成后进列表，v1.116 started
 * 过滤）并从行 title 取 worktree 副本路径（`<会话id> · <worktree_path>`）——
 * 不做 ~/.tenon/worktrees 全局扫描：跨 run 残留会污染断言。
 */
async function worktreeRow(page: Page): Promise<{ row: Locator; worktreePath: string }> {
  const row = page.locator(".pe-chat-item").first();
  await expect(row).toBeVisible({ timeout: 30_000 });
  const title = await row.locator(".pe-chat-row").getAttribute("title");
  const worktreePath = title?.split(" · ")[1] ?? "";
  expect(worktreePath, "会话行 title 应携带受管 worktree 路径").toBeTruthy();
  return { row, worktreePath };
}

test("managed worktree write stays off the main root until merge folds it in", async ({
  page,
}) => {
  test.setTimeout(150_000);
  await openManagedDraft(page);
  await runWriteTask(page, "e2e-wt.txt", "written in managed worktree\n");

  // 会话态上下文条出现 ⎇ 受管徽标（worktree_path 非空即受管，v1.126）；
  // 断言只锁 testid 与 ⎇ 符号——文案随语言（ui-prefs 跨 spec 共享）
  const badge = page.getByTestId("session-worktree-badge");
  await expect(badge).toBeVisible();
  await expect(badge).toContainText("⎇");

  // 写隔离：文件只落 worktree 副本（行 title 给出精确路径），主根保持干净（§9.7 写边界）
  const rootCopy = path.join(runtime.projectA, "e2e-wt.txt");
  await expect
    .poll(async () => {
      try {
        await readFile(rootCopy, "utf8");
        return "root polluted";
      } catch {
        return "absent";
      }
    }, { timeout: 10_000 })
    .toBe("absent");
  const { row, worktreePath } = await worktreeRow(page);
  await expect(readFile(path.join(worktreePath, "e2e-wt.txt"), "utf8")).resolves.toContain(
    "written in managed worktree"
  );

  // 行尾合并 / 丢弃收尾钮 hover 才显示（pe-row-actions display:none →
  // hover/focus-within）；定位用类名不取文案——按钮文案随语言
  await row.hover();
  const merge = row.locator(".pe-row-actions .pe-action:not(.danger)");
  await expect(merge).toBeVisible({ timeout: 20_000 });

  // 合并：文件三方并入主根，内容忠实
  await merge.click();
  await expect
    .poll(async () => readFile(rootCopy, "utf8").catch(() => ""), { timeout: 30_000 })
    .toContain("written in managed worktree");

  // 收尾完成：worktree 目录与快照分片随之清除（store 行内路径保留仅为记录，
  // ⎇ 徽标与收尾钮随之常驻——重合并幂等 already_applied，目录不存在即 409 守卫）
  await expect
    .poll(async () => {
      try {
        await readFile(path.join(worktreePath, "e2e-wt.txt"), "utf8");
        return "worktree copy remains";
      } catch {
        return "cleaned";
      }
    }, { timeout: 30_000 })
    .toBe("cleaned");
  await expect(row).toContainText("⎇");
});

test("discard deletes the worktree and the main root never sees the change", async ({
  page,
}) => {
  test.setTimeout(150_000);
  await openManagedDraft(page);
  await runWriteTask(page, "e2e-wt-discard.txt", "doomed worktree change\n");
  const { row, worktreePath } = await worktreeRow(page);
  const copy = path.join(worktreePath, "e2e-wt-discard.txt");
  await expect(readFile(copy, "utf8")).resolves.toContain("doomed worktree change");

  // 丢弃带原生 confirm（未合并改动即删警示）——Playwright 惯例：事件先行
  page.on("dialog", (dialog) => void dialog.accept());
  await row.hover();
  const discard = row.locator(".pe-row-actions .pe-action.danger");
  await expect(discard).toBeVisible({ timeout: 20_000 });
  await discard.click();

  // 收尾：worktree 目录清除（行内路径保留仅为记录）、主根自始至终未见该改动
  await expect
    .poll(async () => {
      try {
        await readFile(copy, "utf8");
        return "worktree copy remains";
      } catch {
        return "cleaned";
      }
    }, { timeout: 30_000 })
    .toBe("cleaned");
  const rootCopy = path.join(runtime.projectA, "e2e-wt-discard.txt");
  await expect
    .poll(async () => {
      try {
        await readFile(rootCopy, "utf8");
        return "root polluted";
      } catch {
        return "absent";
      }
    }, { timeout: 10_000 })
    .toBe("absent");
});
