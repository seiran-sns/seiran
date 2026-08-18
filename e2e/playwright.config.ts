import { defineConfig } from "@playwright/test";
import path from "node:path";
import { BACKEND_PORT as backendPort, FRONTEND_PORT as frontendPort, PLC_STUB_PORT as plcStubPort, APPVIEW_STUB_PORT as appviewStubPort, LOCAL_DOMAIN as localDomain } from "./ports.ts";

const e2eDir = path.dirname(new URL(import.meta.url).pathname);
const repoRoot = path.resolve(e2eDir, "..");
const tsxBin = path.join(e2eDir, "node_modules", ".bin", "tsx");

// 【重要・変更禁止】webServer 各エントリの reuseExistingServer は必ず false にすること。
// backendPort/frontendPort は ports.ts で scripts/dev-up.sh のネイティブ開発サーバー
// （3000/5173）とは別のポート（3100/5273）に意図的に分離しているため、E2E実行中も
// 開発サーバーを止めなくてよい。ただし reuseExistingServer を true にすると、万一
// 手元で同じポートに何か別プロセスが立っていた場合にそれへ無条件で相乗りしてしまう。
// 2026-07-20、開発サーバーとポートが重複した状態でこれが起き、本物の開発DB・本物の
// plc.directory・本物のBsky Relayに接続している開発サーバーにE2Eが相乗りし、開発DBに
// 48件のテストユーザーが混入・実PLCディレクトリを汚染する事故になった。
// false ならポート競合時に明確なエラーで止まるので安全。

// バックエンドが `cargo run` 起動時に dotenvy でリポジトリルートの実 .env を読み込むため、
// ここで明示的に上書きしない値（REDIS_URL 以外）は本物の .env の値が漏れてくる。
// E2E が本物の外部サービス（Bsky Relay・Cloudflare DNS等）に触れないよう、
// 関係する変数は必ずここで明示的に上書き/空値にする。
const backendEnv: Record<string, string> = {
  POSTGRES_USER: "seiran_e2e",
  POSTGRES_PASSWORD: "seiran_e2e",
  POSTGRES_DB: "seiran_e2e",
  DB_HOST: "localhost",
  DB_PORT: "5433",
  PORT: String(backendPort),
  LOCAL_DOMAIN: localDomain,
  // OGP付きの直リンクHTMLをAPIが取得する先。未指定だとDocker向けの
  // http://frontend:5173 を参照して502になり、/notes/:id・/@userのE2Eが空になる。
  FRONTEND_ORIGIN: `http://127.0.0.1:${frontendPort}`,
  WEBAUTHN_ORIGIN: `http://localhost:${frontendPort}`,
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

// 全specが同じE2E DBを共有するが、実際にグローバル状態（site_settings・
// appviewスタブのプロセスグローバルなsearchRequests・storage_providersの
// アクティブプロバイダ選択順）に触れるのは以下の6ファイルだけで、残りは自前ユーザー・
// 動的ポートstubで完結し並列安全（調査・検証済み）。
//
// - storage_providers: バックエンドの `list_active` が `WHERE is_active = true
//   ORDER BY id` の先頭プロバイダを全アップロードに使うため、複数specが同時に
//   stub S3 を登録すると相互に取り違える。3specはproject内で直列にしつつ、
//   `main` とはインターリーブする。
const STORAGE_SERIAL_SPECS = [
  "**/notifications.spec.ts",
  "**/misskey-compat.spec.ts",
  "**/federation-delivery.spec.ts",
];
// - site_settings（rate-limit/admin）・appviewスタブのグローバルsearchRequests
//   （search）は他specを巻き込んで壊すため、全テスト完了後の排他テールで実行する。
const GLOBALS_SERIAL_SPECS = [
  "**/admin.spec.ts",
  "**/rate-limit.spec.ts",
  "**/search.spec.ts",
];

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: process.env.CI ? 3 : 4,
  // 3プロジェクト・2フェーズ構成（詳細は各projectのコメント参照）。
  // 【注意】`dependencies` はCLIのファイルフィルタを無視するため、ローカルで
  // 例えば `npx playwright test tests/search.spec.ts` と単体実行すると
  // main/storage-serial が先に全部走ってしまう。単体実行したい場合は
  // `--no-deps` を付けること。
  projects: [
    {
      name: "main",
      testIgnore: [...STORAGE_SERIAL_SPECS, ...GLOBALS_SERIAL_SPECS],
    },
    {
      // project内は workers:1 でファイル直列（同じstorage_providersを取り合わない）。
      // dependenciesチェーンにはしない（Playwrightのフェーズ実行は前フェーズ全体の
      // 完了を待つ逐次実行のため、チェーンにすると main とインターリーブされず
      // 丸ごと直列テールに落ちてしまう。per-project workers:1 なら同一フェーズ内で
      // 他projectとインターリーブしながら自分のspecだけ1つずつ実行できる）。
      name: "storage-serial",
      workers: 1,
      testMatch: STORAGE_SERIAL_SPECS,
    },
    {
      // main・storage-serial の完了を待ってから排他実行する（フェーズ2）。
      // テール内の3ファイルの実行順は任意で安全（例えばrate-limitが検索APIを
      // 複数回叩いても、search.specは自分のテストの直前でappviewスタブの
      // searchRequestsをリセットしてから検証する）。
      name: "globals-serial",
      workers: 1,
      testMatch: GLOBALS_SERIAL_SPECS,
      dependencies: ["main", "storage-serial"],
    },
  ],
  // dm.spec.ts等、複数の expect.poll/toBeVisible(timeout:15_000) を逐次連結するテストが
  // デフォルトの30秒制限に対して余裕が無くflakyになりうるため明示的に延長する。
  timeout: 60_000,
  // CIの共有runnerでのみ、外部プロセス起動やWebSocket切断タイミング由来の一過性失敗を
  // trace付きで1回再試行する。ローカルでは失敗を即座に見せる。
  retries: process.env.CI ? 1 : 0,
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
      command: `${tsxBin} fixtures/stub-plc-server.ts`,
      cwd: e2eDir,
      env: { PLC_STUB_PORT: String(plcStubPort) },
      port: plcStubPort,
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
    {
      command: `${tsxBin} fixtures/stub-appview-server.ts`,
      cwd: e2eDir,
      env: { APPVIEW_STUB_PORT: String(appviewStubPort) },
      port: appviewStubPort,
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
    {
      command: `${tsxBin} ${path.join(e2eDir, "scripts", "wait-for-db.ts")} && cargo run -p seiran-server`,
      cwd: repoRoot,
      env: backendEnv,
      port: backendPort,
      // CIのクリーンrunnerではRust workspaceの初回ビルドだけで3分を超える。
      // job全体はGitHub Actions側で30分に制限しつつ、backend起動には余裕を持たせる。
      timeout: 600_000,
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
    {
      command: "npm run dev",
      cwd: path.join(repoRoot, "frontend"),
      env: { FRONTEND_PORT: String(frontendPort), BACKEND_PORT: String(backendPort) },
      port: frontendPort,
      reuseExistingServer: false, // 変更禁止・理由は上部コメント参照
    },
  ],
});
