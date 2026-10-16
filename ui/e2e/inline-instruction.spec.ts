// 行内指令动线 E2E（§7.4 / §8.5 行内指令——Agent ↔ 编辑器共生）：选区上下文
// 组装任务（Cmd/Ctrl+I 浮层）→ 发送当前会话（草稿首发建会话）→ 代理改写打开
// 中的文件 → watcher 回读进编辑器（缓冲非脏不吞内容）+ AI 角标（§8.6）。
// 此前该动线仅 vitest（buildInlineTask 组装单测）覆盖，「选区 → 指令 → 代理
// 改写 → 回读 + 角标」的浏览器级串联无回归。
// fixture 纪律：E2E_WRITE 会整体覆写 a.txt，收尾按测试起始内容还原——后行
// search-panel 断言 a.txt 恰有一个 "alpha" 命中。
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";

interface Runtime {
  baseUrl: string;
  projectA: string;
}

let runtime: Runtime;
const mod = process.platform === "darwin" ? "Meta" : "Control";

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json"
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

/** Cmd/Ctrl+P 打开 a.txt（file-finder spec 同款动线）。 */
async function openAFile(page: Page) {
  await page.keyboard.press(`${mod}+P`);
  const input = page.getByTestId("file-finder-input");
  await expect(input).toBeFocused();
  await input.fill("a.txt");
  // 等查找结果就绪（120ms 防抖）再回车——防空态回车不开启
  await expect(page.getByTestId("file-finder").locator(".finder-path").first()).toHaveText(
    "a.txt"
  );
  await input.press("Enter");
  await expect(page.locator(".editor-overlay")).toBeVisible({ timeout: 15_000 });
  await expect(page.locator(".editor-overlay .view-lines")).toContainText("alpha");
}

test("inline instruction sends selection context and the agent edit lands back in the editor", async ({
  page,
}) => {
  test.setTimeout(150_000);
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();
  // 自隔离（套件纪律）：前置 spec 会留下带工具足迹的活跃会话——mock 驱动
  // 对有工具结果的会话首轮即直出收尾文本（hasToolResult 启发式），写入类
  // 标记必须跑在新鲜会话里才走 apply_patch 管线。
  await page.getByTestId("project-new-task").click();
  const target = path.join(runtime.projectA, "a.txt");
  const original = await readFile(target, "utf8");

  await openAFile(page);

  // 全选构造选区（range 模式）→ Cmd/Ctrl+I 打行内指令浮层
  const overlay = page.locator(".editor-overlay");
  await overlay.locator(".view-lines").click();
  await page.keyboard.press(`${mod}+A`);
  await page.keyboard.press(`${mod}+I`);
  await expect(page.getByTestId("inline-instruction")).toBeVisible();
  // 上下文条带目标文件路径（文案随语言，只锁路径）
  await expect(page.getByTestId("inline-context")).toContainText("a.txt");

  // Esc 先关（焦点落弹层输入框——弹层自身消费 Esc 并 preventDefault，不得穿透误暂停代理）
  await page.getByTestId("inline-input").click();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("inline-instruction")).toHaveCount(0);

  // 重开并发送：指令携带 mock 标记（E2E_WRITE 正则不锚定，任务文本任意
  // 位置可命中）——代理把打开中的 a.txt 整体改写
  await page.keyboard.press(`${mod}+I`);
  await expect(page.getByTestId("inline-instruction")).toBeVisible();
  const b64 = Buffer.from("INLINE-GAMMA\n").toString("base64");
  await page
    .getByTestId("inline-input")
    .fill(["E2E_WRITE", "a.txt", b64].join("\n"));
  await page.getByTestId("inline-input").press("Enter");
  await expect(page.getByTestId("inline-instruction")).toHaveCount(0);

  // 回合完成（草稿首发建会话 → mock 管线真实落盘）
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", {
    timeout: 90_000,
  });
  await expect
    .poll(async () => readFile(target, "utf8").catch(() => ""), { timeout: 15_000 })
    .toContain("INLINE-GAMMA");

  // watcher 回读：打开中的 tab 非脏，磁盘内容回放进编辑器
  await expect(overlay.locator(".view-lines")).toContainText("INLINE-GAMMA", {
    timeout: 15_000,
  });

  // AI 角标（§8.6）：代理改动行挂 .ai-line 装饰 + 行首 glyph
  await expect(overlay.locator(".ai-line").first()).toBeVisible({ timeout: 15_000 });

  // 还原 fixture（不污染后行 search-panel 的 a.txt 命中数断言）
  await writeFile(target, original);
  await expect
    .poll(async () => readFile(target, "utf8").catch(() => ""), { timeout: 10_000 })
    .toBe(original);
});
