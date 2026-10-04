import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      {
        // jsdom 测试不加载 monaco worker（vite ?worker 导入仅浏览器可用）；
        // 全匹配替换，避免相对前缀残留。
        find: /.*monacoSetup$/,
        replacement: new URL("./src/monacoSetupStub.ts", import.meta.url).pathname,
      },
    ],
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    // e2e/ 是 @playwright/test 套件（ui:e2e），vitest 不收集
    exclude: ["**/node_modules/**", "e2e/**"],
  },
});
