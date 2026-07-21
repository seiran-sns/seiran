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
        // /notes/:id・/@handle は常にバックエンドへ転送する。バックエンドが Accept
        // ヘッダーで AP JSON-LD / OGP注入済み SPA HTML を出し分ける
        // （`crates/seiran-api/src/handlers/ogp.rs`）。OGP 注入時はバックエンドが
        // ルート `/` を取得しに来るだけなのでここには来ず、循環しない。
        // `/@` は単純なプレフィックスマッチだと Vite 自身の内部モジュール
        // （`/@vite/client`・`/@react-refresh`・`/@fs/...`・`/@id/...`）まで
        // バックエンドへ転送してしまい、Viteクライアントが読み込めず白画面になる
        // （実機確認）。それらを除外する正規表現（`^`始まりはVite側でregex扱い）にする。
        "/notes": "http://localhost:3000",
        "^/@(?!vite|react-refresh|fs/|id/)": "http://localhost:3000",
      },
    },
    test: {
      environment: "jsdom",
    },
  };
});
