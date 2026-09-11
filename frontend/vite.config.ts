import { readFileSync } from "node:fs";
import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

// .env はリポジトリルート（frontend/ の一つ上）に置かれている。
const repoRoot = path.resolve(import.meta.dirname, "..");

// frontend/backend共通のシステムバージョン（docs/architecture.md 2.1節）。
// `package.json`の`version`+`versionSuffix`を唯一の情報源とし、ビルド時定数へ埋め込む
// （`src/version.ts`の`__FRONTEND_VERSION__`）。`versionSuffix`はnpmが関知しない
// 独自キーで、フィーチャーブランチ・フォークでの運用のため`version`本体とは別行に
// 分離している（Cargo側の`version_suffix`と同じ理由、`Cargo.toml`のコメント参照）。
const pkg = JSON.parse(readFileSync(path.resolve(import.meta.dirname, "package.json"), "utf-8"));
const frontendVersion = `${pkg.version}${pkg.versionSuffix ?? ""}`;

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, repoRoot, "");

  // E2E（e2e/playwright.config.ts）は scripts/dev-up.sh のネイティブ開発サーバー
  // （5173・バックエンド3000）を止めずに済むよう、FRONTEND_PORT/BACKEND_PORT で
  // ポートを別値に上書きできるようにしている。未設定時は通常の開発時の既定値。
  const frontendPort = Number(env.FRONTEND_PORT ?? "5173");
  const backendTarget = `http://localhost:${env.BACKEND_PORT ?? "3000"}`;
  // エージェント主導の開発では、HMR付きdevサーバー（StrictModeのeffect二重実行等、
  // 開発ビルド固有の挙動が本物のバグの調査を遠回りさせることがある）より、
  // 常時ビルド済み・本番相当のコードを配信するpreviewサーバーを標準にする
  // （`npm run build:watch` + `npm run preview` の2プロセス常駐運用）。
  const previewPort = Number(env.PREVIEW_PORT ?? "4174");
  // "frontend" は Docker Compose 上のコンテナ名。backend の OGP ハンドラ
  // （`crates/seiran-api/src/handlers/ogp.rs`）が `frontend_origin`
  // （既定 `http://frontend:5173`）へ index.html を取りに来る際、そのリクエストの
  // Host ヘッダーがコンテナ名 "frontend" になる。これを allowedHosts に含めないと
  // Vite が「Blocked request. This host ("frontend") is not allowed」を返し、
  // /notes/:id・/@:handle への直接アクセス（OGP注入経路）がすべて壊れる（実機確認）。
  const allowedHosts = [env.LOCAL_DOMAIN ?? "localhost", "frontend"];

  return {
    plugins: [react()],
    define: {
      __FRONTEND_VERSION__: JSON.stringify(frontendVersion),
    },
    server: {
      host: "0.0.0.0",
      port: frontendPort,
      allowedHosts,
      proxy: {
        // ローカル開発（cargo run 直接起動）時のみ有効。
        // Docker + nginx 構成では nginx がルーティングを担うため不使用。
        // ws:true で /api/streaming の WebSocket もプロキシする（#37）。
        "/api": { target: backendTarget, ws: true },
        "/proxy": backendTarget,
        "/miauth": backendTarget,
        // /notes/:id・/@handle は常にバックエンドへ転送する。バックエンドが Accept
        // ヘッダーで AP JSON-LD / OGP注入済み SPA HTML を出し分ける
        // （`crates/seiran-api/src/handlers/ogp.rs`）。OGP 注入時はバックエンドが
        // ルート `/` を取得しに来るだけなのでここには来ず、循環しない。
        // `/@` は単純なプレフィックスマッチだと Vite 自身の内部モジュール
        // （`/@vite/client`・`/@react-refresh`・`/@fs/...`・`/@id/...`）まで
        // バックエンドへ転送してしまい、Viteクライアントが読み込めず白画面になる
        // （実機確認）。それらを除外する正規表現（`^`始まりはVite側でregex扱い）にする。
        "/notes": backendTarget,
        "/announces": backendTarget,
        "^/@(?!vite|react-refresh|fs/|id/)": backendTarget,
      },
    },
    preview: {
      host: "0.0.0.0",
      port: previewPort,
      allowedHosts,
      proxy: {
        "/api": { target: backendTarget, ws: true },
        "/proxy": backendTarget,
        "/miauth": backendTarget,
        "/notes": backendTarget,
        "/announces": backendTarget,
      },
    },
    test: {
      environment: "jsdom",
      // jsdomのデフォルトURL(about:blank)はopaque originとなりlocalStorageが
      // 使えないため、下書き自動保存（#193）のテストのため実URLを与える。
      environmentOptions: {
        jsdom: { url: "http://localhost/" },
      },
    },
  };
});
