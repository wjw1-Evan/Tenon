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
