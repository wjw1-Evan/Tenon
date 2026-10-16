// 编辑器保存动线 E2E（§7 保存 v1.75 双保存模式）：monaco 真实键入 → 自动模式
// 落盘（读路径 flush / 1s 去抖任一路径）+ 刷新回放；手动模式「仅显式保存写盘」
// ——补全 / hover 读路径 flush 不落盘（onReadFlushFile 模式门控），Cmd/Ctrl+S
// 才写盘。此前保存动线仅 vitest（jsdom mock）与 saveNow / AutoSaver 单测覆盖，
// 「编辑器键入 → 落盘 → 圆点语义」的浏览器级串联无回归（file-finder spec 只读）。
import { readFile } from "node:fs/promises";
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
    "../test-results/tenon-runtime.json",
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

/** Cmd/Ctrl+P 打开 a.txt（file-finder spec 同款动线），返回编辑器浮层定位。 */
async function openAFile(page: Page) {
  await page.keyboard.press(`${mod}+P`);
  const input = page.getByTestId("file-finder-input");
  await expect(input).toBeFocused();
  await input.fill("a.txt");
  // 等查找结果就绪（120ms 防抖）再回车——file-finder spec 同款，防空态回车不开启
  await expect(page.getByTestId("file-finder").locator(".finder-path").first()).toHaveText(
    "a.txt"
  );
  await input.press("Enter");
  const overlay = page.locator(".editor-overlay");
  await expect(overlay).toBeVisible({ timeout: 15_000 });
  await expect(overlay.locator(".view-lines")).toContainText("alpha");
  return overlay;
}

/** 首屏就绪（项目行出现即 App 完成 boot）。 */
async function boot(page: Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();
}

/** 点入 monaco 正文并在光标处键入（真编辑器事件链，非直改缓冲）。 */
async function typeIntoEditor(page: Page, overlay: ReturnType<Page["locator"]>, text: string) {
  await overlay.locator(".view-lines").click();
  await page.keyboard.type(text);
  await expect(overlay.locator(".view-lines")).toContainText(text);
}

test("auto save mode lands edits on disk and a reload replays them", async ({ page }) => {
  await boot(page);
  const overlay = await openAFile(page);
  await typeIntoEditor(page, overlay, "TYPED-AUTO");

  // 自动保存管线落盘（读路径 flush 与 1s 去抖任一路径均可）：磁盘内容忠实
  const target = path.join(runtime.projectA, "a.txt");
  await expect
    .poll(async () => readFile(target, "utf8").catch(() => ""), { timeout: 15_000 })
    .toContain("TYPED-AUTO");
  // 保存成功即解除未保存圆点（终态稳定；亮起瞬间与读路径 flush 竞速，不作可见断言）
  await expect(page.getByTestId("unsaved-a.txt")).toBeHidden();

  // 刷新重开：读路径回放已落盘内容（落盘不是幻觉）
  await page.reload();
  await boot(page);
  await openAFile(page);
  await expect(page.locator(".editor-overlay .view-lines")).toContainText("TYPED-AUTO");
});

test("manual save mode skips the debounce until Cmd/Ctrl+S", async ({ page }) => {
  // 保存方式切手动（§7.5 即时生效）：同页会话内切换，不重导航——重载会让
  // ui_prefs PUT 与导航竞速，读到 auto 就把本测的「不去抖」断言变成假题。
  await boot(page);
  await page.getByTestId("rail-settings").click();
  const modeSelect = page.getByTestId("settings-save-mode");
  await modeSelect.selectOption("manual");
  await expect(modeSelect).toHaveValue("manual");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("settings-overlay")).toHaveCount(0);

  await openAFile(page);
  await typeIntoEditor(page, page.locator(".editor-overlay"), "TYPED-MANUAL");

  // 手动模式契约（§7 保存 v1.75「仅显式保存写盘」）：圆点亮起且不灭、远超
  // 自动档 1s 去抖窗口后磁盘仍未写入——读路径（补全 / hover）flush 亦不得
  // 落盘，否则手动保存名存实亡（回归锚点：onReadFlushFile 模式门控）
  const dot = page.getByTestId("unsaved-a.txt");
  await expect(dot).toBeVisible();
  const target = path.join(runtime.projectA, "a.txt");
  await page.waitForTimeout(2_500);
  await expect(await readFile(target, "utf8")).not.toContain("TYPED-MANUAL");

  // Cmd/Ctrl+S 显式保存（saveNow：flush 去抖条目 → 写盘 → 圆点熄灭）
  await page.keyboard.press(`${mod}+S`);
  await expect
    .poll(async () => readFile(target, "utf8").catch(() => ""), { timeout: 15_000 })
    .toContain("TYPED-MANUAL");
  await expect(dot).toBeHidden();

  // 还原 auto：保存方式影响编辑器动线，不留跨 spec 副作用（同页切回，状态受控可断言）。
  // 先关编辑器浮层：全屏浮层压在 rail 上方，不关则设置钮点击被 monaco 层拦截
  await page.getByTestId("editor-overlay-close").click();
  await expect(page.getByTestId("editor-overlay")).toHaveCount(0);
  await page.getByTestId("rail-settings").click();
  const restore = page.getByTestId("settings-save-mode");
  await restore.selectOption("auto");
  await expect(restore).toHaveValue("auto");
  await page.keyboard.press("Escape");
});
