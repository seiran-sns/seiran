import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

// .env はリポジトリルート（frontend/ の一つ上）に置かれている。
const repoRoot = path.resolve(import.meta.dirname, "..");

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, repoRoot, "");

  return {
    plugins: [react()],
    server: {
      host: "0.0.0.0",
      port: 5173,
      allowedHosts: [env.LOCAL_DOMAIN ?? "localhost"],
      proxy: {
        // ローカル開発（cargo run 直接起動）時のみ有効。
        // Docker + nginx 構成では nginx がルーティングを担うため不使用。
        // ws:true で /api/streaming の WebSocket もプロキシする（#37）。
        "/api": { target: "http://localhost:3000", ws: true },
        "/miauth": "http://localhost:3000",
        // /notes/:id は AP クライアント向け（Accept: activity+json / ld+json）のみ
        // バックエンドへ転送し、それ以外（ブラウザ）は SPA の NoteDetailPage に委ねる。
        // nginx.conf の $notes_upstream map と同じロジック。
        "/notes": {
          target: "http://localhost:3000",
          bypass(req) {
            const accept = req.headers.accept ?? "";
            if (/application\/(activity|ld)\+json/.test(accept)) {
              return undefined; // バックエンドへプロキシ
            }
            return req.url; // Vite（SPA fallback）に処理させる
          },
        },
      },
    },
  };
});
