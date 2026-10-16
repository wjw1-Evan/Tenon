// 设置面板动线 E2E（§7.2/§15 + §11 密钥不变式，真 daemon + 真 settings.json）：
// ① rail 打开 → Models 分区 provider 表回显 config.toml 合并视图，且明文
//    api_key 在任何 DOM（文本 + 输入框值）都不出现——「GET /settings 永不
//    回显」此前仅 Rust 单测断言响应体，浏览器级不变式无回归；
// ② 默认模型 / 备用链 / 生成参数 PUT 持久化 → 刷新整页后重开仍回显
//    （settings.json 0600 真落盘；provider 条目按字段合并，config.toml 的
//    base_url / api_key 不被设置覆盖抹掉——v1.165 双轨的动线级验证）；
// ③ 保存方式即时切换存 daemon ui_prefs（§7.5）→ 刷新后仍是 manual；
// ④ 模型路由对话框（§11 会话级热切换）+ 路由建议（纯读任务建议轻模型）。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";

interface Runtime {
  baseUrl: string;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json",
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

/** 打开工作区并经 rail 钮进入设置（App 启动即拉 GET /settings，对话框吃该份）。 */
async function openSettings(page: Page, modelsTab = true) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();
  await page.getByTestId("rail-settings").click();
  await expect(page.getByTestId("settings-overlay")).toBeVisible();
  if (modelsTab) {
    await page.getByTestId("settings-nav-models").click();
    await expect(page.getByTestId("settings-models-section")).toBeVisible();
  }
}

test("provider table echoes the merged view and never echoes the stored api key", async ({
  page,
}) => {
  await openSettings(page);

  // config.toml 定义的 mock provider（global-setup 写入）出现在合并视图
  const row = page.getByTestId("provider-row-mock");
  await expect(row).toBeVisible();
  const baseUrl = row.locator("input").first();
  await expect(baseUrl).toHaveValue(/http:\/\/127\.0\.0\.1:\d+\/v1/);

  // §11 密钥不变式：明文 api_key 不出现在任何可见文本或输入框值里
  const dump = await page.evaluate(() =>
    [
      document.body.innerText,
      ...Array.from(
        document.querySelectorAll("input, textarea, select")
      ).map((el) => (el as HTMLInputElement).value ?? ""),
    ].join("\n")
  );
  expect(dump).not.toContain("e2e-local");
});

test("default model, fallback chain and generation params persist across reload", async ({
  page,
}) => {
  await openSettings(page);
  await page.getByTestId("settings-default-model").selectOption("mock");
  await page.getByTestId("settings-fallback-chain").fill("mock");
  await page.getByTestId("settings-gen-max-tokens").fill("4096");
  await page.getByTestId("settings-gen-temperature").fill("0.5");
  await page.getByTestId("settings-save").click();
  await expect(page.getByTestId("settings-overlay")).toBeHidden();

  // 整页刷新后重开：值来自 daemon 的 settings.json 合并视图，而非前端内存
  await page.reload();
  await openSettings(page);
  await expect(page.getByTestId("settings-default-model")).toHaveValue("mock");
  await expect(page.getByTestId("settings-fallback-chain")).toHaveValue("mock");
  await expect(page.getByTestId("settings-gen-max-tokens")).toHaveValue("4096");
  await expect(page.getByTestId("settings-gen-temperature")).toHaveValue("0.5");

  // provider 条目按字段合并：设置覆盖不含密钥字段，config.toml 的 base_url 保留
  const baseUrl = page.getByTestId("provider-row-mock").locator("input").first();
  await expect(baseUrl).toHaveValue(/http:\/\/127\.0\.0\.1:\d+\/v1/);
});

test("save mode switches live and survives a reload via daemon ui_prefs", async ({
  page,
}) => {
  await openSettings(page, false);
  await expect(page.getByTestId("settings-general-section")).toBeVisible();

  // 即时切换（App → PUT ui_prefs，§7.5 权威），无「保存」钮
  await page.getByTestId("settings-save-mode").selectOption("manual");
  await page.reload();
  await openSettings(page, false);
  await expect(page.getByTestId("settings-save-mode")).toHaveValue("manual");

  // 还原 auto：保存方式影响编辑器动线，不留跨 spec 副作用
  await page.getByTestId("settings-save-mode").selectOption("auto");
  await expect(page.getByTestId("settings-save-mode")).toHaveValue("auto");
});

test("model routing dialog hot-switches the session and suggests a light model for read-only tasks", async ({
  page,
}) => {
  test.setTimeout(120_000);
  // 会话级切换需活会话：会话记录随首个任务落库（仅开新任务输入不改
  // sessionId），先跑一个即时收尾的 E2E_REPLY 任务（checkpoint spec 同款
  // 纪律——共享 daemon 下自造数据，不复用既有会话）
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();
  await page.getByTestId("project-new-task").click();
  await page.getByTestId("task-input").fill("E2E_REPLY\nrouting-probe-final-answer");
  await page.getByTestId("send").click();
  await expect(page.getByTestId("agent-feed")).toContainText("routing-probe-final-answer", {
    timeout: 90_000,
  });

  // 触发钮显示当前默认 provider（config.toml models.default = "mock"）
  await expect(page.getByTestId("model-current")).toContainText("mock");
  await page.getByTestId("model-trigger").click();
  await expect(page.getByTestId("model-dialog-overlay")).toBeVisible();

  // provider 列表含 mock 且为当前项（✓ 高亮）；会话存在故可点
  const option = page.getByTestId("model-option-mock");
  await expect(option).toBeEnabled();
  await expect(option).toHaveAttribute("aria-pressed", "true");
  await expect(option).toContainText("mock-1");

  // 路由建议（§11 集成点 #4）：纯读任务（含 explain 关键字）→ 建议轻模型
  await page.getByTestId("suggest-input").fill("explain what this module does");
  await page.getByTestId("suggest-btn").click();
  await expect(page.getByTestId("suggest-result")).toContainText("Read-only task");

  // 会话级热切换（此处为同 provider 重选，验证端点链路与对话框收起）
  await option.click();
  await expect(page.getByTestId("model-dialog-overlay")).toBeHidden();
  await expect(page.getByTestId("model-current")).toContainText("mock");
});
