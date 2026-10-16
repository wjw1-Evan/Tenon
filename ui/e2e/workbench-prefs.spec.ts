// 工作台偏好动线 E2E（§7.5）：外观三态循环按钮（v1.119 单钮：跟随系统 → 浅色
// → 深色）即时落 data-theme；顶栏语言选择器（v1.100 九语言）切简体中文后界面
// 文案与 <html lang> 即时切换（语言包懒加载分块真链路）。两者均双写 daemon
// /ui-prefs（跨启动权威），本 spec 按字母序最后执行，不污染先行 spec 的英文
// 断言。
import { readFile } from "node:fs/promises";
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

test("theme button cycles system → light → dark with data-theme following", async ({
  page,
}) => {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  const picker = page.getByTestId("theme-picker");
  // 新档案 localStorage 为空：初始「跟随系统」，headless Chromium 预浅色
  await expect(picker).toHaveAttribute("data-theme-pref", "system");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  await picker.click();
  await expect(picker).toHaveAttribute("data-theme-pref", "light");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  await picker.click();
  await expect(picker).toHaveAttribute("data-theme-pref", "dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
});

test("language picker switches the UI to Simplified Chinese live", async ({ page }) => {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  // 切简体中文（语言包按需分块拉取）：rail 搜索钮文案即变。
  // 选择器收窄到顶栏 header——"Language" 可访问名在设置面等处可能重名。
  await page.locator("header.app-head select").first().selectOption("zh-CN");
  await expect(page.getByTestId("rail-search")).toHaveAttribute("aria-label", "搜索");
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");

  // 既有中英键集门禁外的抽查：底栏开关 title 随语言（en → zh 同键异文）。
  // 底栏开合经项目 ui-state 持久化——先前 spec（如终端）可能留下展开态，
  // 先收起再断言收起钮文案。
  if (!(await page.getByTestId("bottom-open").count())) {
    await page.getByTestId("bottom-close").click();
  }
  await expect(page.getByTestId("bottom-open")).toHaveAttribute("title", "打开底部面板");
});
