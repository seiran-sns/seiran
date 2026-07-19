import { defineConfig } from "@playwright/test";
import path from "node:path";

const e2eDir = path.dirname(new URL(import.meta.url).pathname);
const repoRoot = path.resolve(e2eDir, "..");

const plcStubPort = Number(process.env.PLC_STUB_PORT ?? "2582");
const backendPort = 3000;
const frontendPort = 5173;

// バックエンドが `cargo run` 起動時に dotenvy でリポジトリルートの実 .env を読み込むため、
// ここで明示的に上書きしない値（REDIS_URL 以外）は本物の .env の値が漏れてくる。
// E2E が本物の外部サービス（Bsky Relay・Cloudflare DNS等）に触れないよう、
// 関係する変数は必ずここで明示的に上書き/空値にする。
const backendEnv: Record<string, string> = {
  DATABASE_URL: "postgres://seiran_e2e:seiran_e2e@localhost:5433/seiran_e2e",
  PORT: String(backendPort),
  LOCAL_DOMAIN: "localhost",
  PLC_DIRECTORY_BASE_URL: `http://127.0.0.1:${plcStubPort}`,
  // Relay への requestCrawl が本物の bsky.network に飛ばないよう、存在しないローカル
  // ポートに向けておく（接続失敗はログに出るだけで登録処理自体は継続する）。
  ATP_RELAY_URL: "http://127.0.0.1:1",
  // Cloudflare DNS 連携はE2Eのスコープ外（docs/architecture.md 9章）。空文字なら
  // 無効化される（crates/seiran-api/src/lib.rs のCloudflareClient初期化条件）。
  CLOUDFLARE_API_TOKEN: "",
  CLOUDFLARE_ZONE_ID: "",
  REDIS_URL: "",
  SEIRAN_CONFIG_DIR: path.join(e2eDir, ".tmp-config"),
};

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  retries: 0,
  reporter: "list",
  globalSetup: "./global-setup.ts",
  globalTeardown: "./global-teardown.ts",
  use: {
    baseURL: `http://localhost:${frontendPort}`,
    trace: "retain-on-failure",
    // フロントは i18next-browser-languagedetector でブラウザのロケールを見て言語を
    // 決める。既定（en-US）のままだとUIが英語表示になりテストの日本語ロケータと
    // 食い違うため、seiranの主要言語である日本語に固定する。
    locale: "ja-JP",
  },
  webServer: [
    {
      command: `node fixtures/stub-plc-server.ts`,
      cwd: e2eDir,
      env: { PLC_STUB_PORT: String(plcStubPort) },
      port: plcStubPort,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: "cargo run -p seiran-server",
      cwd: repoRoot,
      env: backendEnv,
      port: backendPort,
      timeout: 180_000,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: "npm run dev",
      cwd: path.join(repoRoot, "frontend"),
      port: frontendPort,
      reuseExistingServer: !process.env.CI,
    },
  ],
});
