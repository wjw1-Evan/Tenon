import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      {
        find: /.*monacoSetup$/,
        replacement: new URL("./src/monacoSetupStub.ts", import.meta.url).pathname,
      },
    ],
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    exclude: ["**/node_modules/**", "e2e/**"],
    // v2.28 时序加固：默认 5s 在共享机器资源竞争下（外部构建进程抢 CPU）
    // 对 App 级轮询类测试（500ms 事件轮询链）偏紧——实测超时 flake 均为此类；
    // 提到 15s（慢机余量 ×3），性能预算测试不受影响（自带显式预算断言）。
    testTimeout: 15_000,
    hookTimeout: 30_000,
    coverage: {
      // 仅统计 src/ 可单测源码；入口 bootstrap / Monaco worker / 测试文件排除。
      include: ["src/**"],
      exclude: [
        "src/main.tsx",
        "src/monacoSetup.ts",
        "src/monacoSetupStub.ts",
        "src/vite-env.d.ts",
        "src/__tests__/**",
      ],
    },
  },
});
