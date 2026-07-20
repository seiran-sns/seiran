// UIログイン以外のセットアップ（テスト対象でないユーザー作成等）はAPIを直接叩いて済ませ、
// 各テストは検証したいUI操作だけに集中させる。

import { APIRequestContext, Page, expect } from "@playwright/test";

export interface E2eUser {
  username: string;
  password: string;
  email: string;
  token: string;
  userId: number;
}

let counter = 0;

function uniqueUsername(prefix: string): string {
  counter += 1;
  return `${prefix}${Date.now().toString(36)}${counter}`;
}

/** requireEmailVerification=false（E2E専用DBの既定値）を前提にした直接登録。 */
export async function registerUserViaApi(
  request: APIRequestContext,
  usernamePrefix = "e2e",
): Promise<E2eUser> {
  const username = uniqueUsername(usernamePrefix);
  const password = "seiranda-e2e";
  const email = `${username}@example.com`;

  const res = await request.post("/api/auth/register", {
    data: { username, password, email },
  });
  expect(res.ok(), `register failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  const body = await res.json();

  return { username, password, email, token: body.token as string, userId: body.user.id as number };
}

/** ページ読み込み前にlocalStorageへtokenを仕込み、UIログイン操作を省略してログイン状態にする。 */
export async function seedAuth(page: Page, token: string): Promise<void> {
  await page.addInitScript((t) => {
    window.localStorage.setItem("seiran_token", t);
  }, token);
}
