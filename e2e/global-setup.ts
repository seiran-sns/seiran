// フロントは api.setup.status()（GET /api/setup/status、users テーブルが1件でも
// あれば initialized:true）をアプリ起動時に1回だけ見て、初期化前なら常に
// <Setup>（初期管理者作成）画面をルーティング無視で表示する
// (frontend/src/App.tsx の AppRoutes)。E2E専用DBは各テスト実行のたびに空から
// 始まるため、何もしないと最初にブラウザでどのURLを開いても<Setup>画面になってしまう。
//
// ここ（globalSetup）は Playwright の実行順序上、webServer（backend含む）が
// 起動・readyになった後に実行される（playwright.config.ts の該当コメント参照）ため、
// backendへ直接HTTPで先に1人登録しておくことで、以降の全テストを通常の画面遷移で
// 開始できるようにする。
import { BACKEND_URL } from "./ports.ts";

export default async function globalSetup() {
  const statusRes = await fetch(`${BACKEND_URL}/api/setup/status`);
  const status = (await statusRes.json()) as { initialized: boolean };
  if (!status.initialized) await bootstrapAdmin();
  await disableAccountLimit();
}

/** 既定のユーザー数上限（100）はE2Eが作る大量のユーザーで超えるため、無制限（0）にしておく。
 * 上限到達時の挙動は`account-limit.spec.ts`が自前で上限を下げて検証する。 */
async function disableAccountLimit() {
  const loginRes = await fetch(`${BACKEND_URL}/api/auth/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ identifier: "e2ebootstrap", password: ADMIN_BOOTSTRAP_PASSWORD }),
  });
  if (!loginRes.ok) throw new Error(`管理者ログインに失敗しました: ${loginRes.status} ${await loginRes.text()}`);
  const { token } = (await loginRes.json()) as { token: string };
  const res = await fetch(`${BACKEND_URL}/api/admin/site-settings`, {
    method: "PATCH",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ max_user_accounts: "0" }),
  });
  if (!res.ok) throw new Error(`ユーザー数上限の解除に失敗しました: ${res.status} ${await res.text()}`);
}

const ADMIN_BOOTSTRAP_PASSWORD = "seiranda-e2e-admin-bootstrap";

async function bootstrapAdmin() {

  const res = await fetch(`${BACKEND_URL}/api/setup`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      username: "e2ebootstrap",
      email: "e2ebootstrap@example.com",
      // `registerUserViaApi`の全テスト共通パスワード（"seiranda-e2e"）とは別の値にする。
      // ログインブルートフォース判定の「同一パスワードへの異なるユーザー名試行数」
      // カウンター（`variants_by_secret`、`crates/seiran-api/src/rate_limit.rs`）は
      // ログイン成功時にもリセットされないため、共通パスワードを使う大量の一般ユーザー
      // ログイン試行に紛れてadmin自身のログインまで拒否されてしまう。
      password: ADMIN_BOOTSTRAP_PASSWORD,
    }),
  });
  if (!res.ok && res.status !== 409) {
    throw new Error(`初期管理者の作成に失敗しました: ${res.status} ${await res.text()}`);
  }
}
