// UIログイン以外のセットアップ（テスト対象でないユーザー作成等）はAPIを直接叩いて済ませ、
// 各テストは検証したいUI操作だけに集中させる。

import { APIRequestContext, Page, expect } from "@playwright/test";

export interface E2eUser {
  username: string;
  password: string;
  email: string;
  token: string;
  userId: number;
  /**
   * actor_id（文字列）。`/api/auth/register` レスポンスの `user.actor_id` は
   * バックエンドで i64 のまま返るため、JS の Number 精度（53bit）を超えて丸め誤差が
   * 生じうる（既存の設計課題、DM機能のE2E実装で顕在化）。他人への宛先指定など
   * actor_id を値として送る用途では、必ずこの文字列版（`/api/actors/search` 経由、
   * 他のAPIレスポンス同様に文字列でシリアライズされる）を使うこと。
   */
  actorId: string;
}

let counter = 0;

// `Date.now()` + プロセス内カウンタだけだと、E2Eの並列化（複数workerプロセス）で
// 別workerが同一ミリ秒に同じprefixで登録した場合に衝突しうる。process.pidを混ぜて
// worker間でも一意にする（usernameは63バイト上限、通常のprefix長なら十分収まる）。
function uniqueUsername(prefix: string): string {
  counter += 1;
  return `${prefix}${Date.now().toString(36)}${process.pid.toString(36)}${counter}`;
}

/** requireEmailVerification=false（E2E専用DBの既定値）を前提にした直接登録。 */
export async function registerUserViaApi(
  request: APIRequestContext,
  usernamePrefix = "e2e",
): Promise<E2eUser> {
  return registerUserWithPasswordViaApi(request, usernamePrefix, "seiranda-e2e");
}

/**
 * `registerUserViaApi`と同様だが、パスワードを呼び出し側が指定できる。
 * `registerUserViaApi`は全テスト共通の固定パスワードを使うため、ログイン
 * ブルートフォース対策のE2E（`rate-limit.spec.ts`）のように「同一パスワードに対する
 * 異なるユーザー名の試行種類数」（逆方向判定）を検証する場合、他テストが同じ
 * パスワードでログイン試行しているとカウントが汚染される。そのため一意パスワードで
 * 隔離したいテストはこちらを使うこと。
 */
export async function registerUserWithPasswordViaApi(
  request: APIRequestContext,
  usernamePrefix: string,
  password: string,
): Promise<E2eUser> {
  const username = uniqueUsername(usernamePrefix);
  const email = `${username}@example.com`;

  const res = await request.post("/api/auth/register", {
    data: { username, password, email },
  });
  expect(res.ok(), `register failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  const body = await res.json();
  const token = body.token as string;

  // body.user.actor_id はバックエンドがi64のまま返すためJS Number精度で丸め誤差が
  // 生じうる（E2eUser.actorIdのコメント参照）。actor_search経由の文字列版で取り直す。
  const searchRes = await request.get(`/api/actors/search?q=${encodeURIComponent(username)}`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(searchRes.ok(), `actor search failed: ${searchRes.status()} ${await searchRes.text()}`).toBeTruthy();
  const suggestions = (await searchRes.json()) as { actor_id: string; username: string }[];
  const self = suggestions.find((s) => s.username === username);
  expect(self, `actor_search で自分自身(${username})が見つからない`).toBeTruthy();

  return {
    username,
    password,
    email,
    token,
    userId: body.user.id as number,
    actorId: self!.actor_id,
  };
}

/**
 * 既存ユーザーとしてAPIログインしtokenを取得する。管理者(`global-setup.ts`が作成する
 * `e2ebootstrap`)等、`registerUserViaApi`を経由しないアカウントでログイン状態を得たい場合に使う。
 */
export async function loginViaApi(request: APIRequestContext, identifier: string, password: string): Promise<string> {
  const res = await request.post("/api/auth/login", { data: { identifier, password } });
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  const body = await res.json();
  return body.token as string;
}

/** ページ読み込み前にlocalStorageへtokenを仕込み、UIログイン操作を省略してログイン状態にする。 */
export async function seedAuth(page: Page, token: string): Promise<void> {
  await page.addInitScript((t) => {
    window.localStorage.setItem("seiran_token", t);
  }, token);
}

// global-setup.ts が作成する初期管理者アカウント（role=admin）。
export const ADMIN_USERNAME = "e2ebootstrap";
export const ADMIN_PASSWORD = "seiranda-e2e";

/**
 * 管理画面のサイト設定をAPI経由で更新する（レート制限系E2Eで閾値を一時的に下げるため）。
 * `workers: 1`（playwright.config.ts）で全テストが直列実行される前提のグローバル設定変更
 * のため、呼び出し側は必ず `try/finally` で元の値へ戻すこと。
 */
export async function patchSiteSettings(
  request: APIRequestContext,
  adminToken: string,
  patch: Record<string, string>,
): Promise<void> {
  const res = await request.patch("/api/admin/site-settings", {
    headers: { Authorization: `Bearer ${adminToken}` },
    data: patch,
  });
  expect(res.ok(), `site-settings更新失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
}

/**
 * 自分自身の AT Protocol DID（`at_did`）を取得する。`GET /api/users/profile?q={username}`
 * のレスポンス（`ProfileResponse.at_did`、`crates/seiran-api/src/handlers/users.rs`）から
 * 取り出すだけ。DID は Bsky側フォロワーポーリング（`getFollowers`）・`subscribeRepos`
 * 購読の対象アクター指定に使う。
 */
export async function getOwnDid(request: APIRequestContext, token: string, username: string): Promise<string> {
  const res = await request.get(`/api/users/profile?q=${encodeURIComponent(username)}`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(res.ok(), `profile取得failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  const body = await res.json();
  const atDid = body.at_did as string | null;
  expect(atDid, `${username} の at_did が未設定`).toBeTruthy();
  return atDid!;
}
