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
  if (status.initialized) return;

  const res = await fetch(`${BACKEND_URL}/api/setup`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      username: "e2ebootstrap",
      email: "e2ebootstrap@example.com",
      password: "seiranda-e2e",
    }),
  });
  if (!res.ok && res.status !== 409) {
    throw new Error(`初期管理者の作成に失敗しました: ${res.status} ${await res.text()}`);
  }
}
