import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    // daemon 固定 127.0.0.1；开发期经代理转发避免 CORS
    proxy: {
      "/api": {
        target: "http://127.0.0.1:9876",
        rewrite: (p) => p.replace(/^\/api/, ""),
      },
    },
  },
});
