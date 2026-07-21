import { defineConfig } from "@playwright/test";
import path from "node:path";

const e2eDir = path.dirname(new URL(import.meta.url).pathname);
const repoRoot = path.resolve(e2eDir, "..");

const plcStubPort = Number(process.env.PLC_STUB_PORT ?? "2582");
const appviewStubPort = Number(process.env.APPVIEW_STUB_PORT ?? "2583");
const backendPort = 3000;
const frontendPort = 5173;

// 【重要・変更禁止】webServer 各エントリの reuseExistingServer は必ず false にすること。
// backendPort(3000)/frontendPort(5173) は scripts/dev-up.sh のネイティブ開発サーバーとも
// 共有しているため、true にすると「既に起動している別プロセス」を無条件に流用してしまう。
// 2026-07-20、まさにこれが起きて本物の開発サーバー（本物の開発DB・本物のplc.directory・
// 本物のBsky Relayに接続）にE2Eが相乗りし、開発DBに48件のテストユーザーが混入・実PLC
// ディレクトリを汚染する事故になった。false ならポート競合時に明確なエラーで止まるので安全。

// バックエンドが `cargo run` 起動時に dotenvy でリポジトリルートの実 .env を読み込むため、
// ここで明示的に上書きしない値（REDIS_URL 以外）は本物の .env の値が漏れてくる。
// E2E が本物の外部サービス（Bsky Relay・Cloudflare DNS等）に触れないよう、
// 関係する変数は必ずここで明示的に上書き/空値にする。
const backendEnv: Record<string, string> = {
  DATABASE_URL: "postgres://seiran_e2e:seiran_e2e@localhost:5433/seiran_e2e",
  PORT: String(backendPort),
  LOCAL_DOMAIN: "localhost",
  PLC_DIRECTORY_BASE_URL: `http://127.0.0.1:${plcStubPort}`,
  ATP_APPVIEW_URL: `http://127.0.0.1:${appviewStubPort}`,
  // Relay への requestCrawl が本物の bsky.network に飛ばないよう、存在しないローカル
  // ポートに向けておく（接続失敗はログに出るだけで登録処理自体は継続する）。
  ATP_RELAY_URL: "http://127.0.0.1:1",
  // Cloudflare DNS 連携はE2Eのスコープ外（docs/architecture.md 9章）。空文字なら
  // 無効化される（crates/seiran-api/src/lib.rs のCloudflareClient初期化条件）。
  CLOUDFLARE_API_TOKEN: "",
  CLOUDFLARE_ZONE_ID: "",
  REDIS_URL: "",
  // Bsky側フォロワー検知ポーリング（`bsky_follower_poll`）の間隔。デフォルト60秒だと
  // E2Eのタイムアウト（15秒）内に検知されないため短縮する。
  BSKY_FOLLOWER_POLL_INTERVAL_SECS: "2",
  SEIRAN_CONFIG_DIR: path.join(e2eDir, ".tmp-config"),
  // sqlx::query! はコンパイル時にDBへ接続してスキーマ検証する。E2E専用DBはこの時点では
  // マイグレーション未適用（マイグレーションはbackend起動時に自動実行される）なので、
  // 生DBへ接続すると「relation "xxx" does not exist」でビルド自体が失敗する。
  // Dockerfileと同様にコミット済みの .sqlx/ オフラインキャッシュを使わせる。
  SQLX_OFFLINE: "true",
};

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  // dm.spec.ts等、複数の expect.poll/toBeVisible(timeout:15_000) を逐次連結するテストが
  // デフォルトの30秒制限に対して余裕が無くflakyになりうるため明示的に延長する。
  timeout: 60_000,
  retries: 0,
  reporter: "list",
  // 【重要】Playwrightの実際の実行順序は「webServer起動 → globalSetup」であり、直感に反する
  // （node_modules/playwright/lib/runner/index.js の createGlobalSetupTasks を見ると
  // webServer は plugin として globalSetups より前に起動される）。そのため:
  // - E2E用DBの起動待ちは globalSetup ではなく scripts/wait-for-db.ts として
  //   backend の command 自体の前段に組み込んでいる（DBはbackend起動前に必要なため）。
  // - globalSetup は逆に「backendが起動済みであること」を前提にできるので、
  //   初期管理者アカウントのbootstrap（global-setup.ts参照）に使っている。
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
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
    {
      command: `node fixtures/stub-appview-server.ts`,
      cwd: e2eDir,
      env: { APPVIEW_STUB_PORT: String(appviewStubPort) },
      port: appviewStubPort,
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
    {
      command: `node ${path.join(e2eDir, "scripts", "wait-for-db.ts")} && cargo run -p seiran-server`,
      cwd: repoRoot,
      env: backendEnv,
      port: backendPort,
      timeout: 180_000,
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
    {
      command: "npm run dev",
      cwd: path.join(repoRoot, "frontend"),
      port: frontendPort,
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
  ],
});
