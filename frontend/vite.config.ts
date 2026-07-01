import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    host: "0.0.0.0",
    port: 5173,
    allowedHosts: ["beta.seiran.org"],
    proxy: {
      // ローカル開発（cargo run 直接起動）時のみ有効。
      // Docker + nginx 構成では nginx がルーティングを担うため不使用。
      "/api": "http://localhost:3000",
      "/miauth": "http://localhost:3000",
    },
  },
});
