// AT Protocol のアプリパスワードの発行・無効化（設定画面と com.atproto.server.*AppPassword）。
// アプリパスワード前提の Bluesky クライアントがログインでき、無効化すればそのセッションも
// 更新できなくなることを確かめる。

import { test, expect, type APIRequestContext } from "@playwright/test";
import {
  getOwnDid,
  registerUserViaApi,
  registerUserWithPasswordViaApi,
  seedAuth,
} from "../fixtures/api-helpers";
import { BACKEND_URL } from "../ports.ts";

async function createSession(request: APIRequestContext, identifier: string, password: string) {
  const res = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.createSession`, {
    data: { identifier, password },
  });
  expect(res.ok(), `createSession失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()) as { accessJwt: string; refreshJwt: string };
}

const bearer = (jwt: string) => ({ Authorization: `Bearer ${jwt}` });

test("設定画面でアプリパスワードを発行してログインでき、無効化するとセッションを更新できなくなる", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2eapppwui");
  const did = await getOwnDid(request, user.token, user.username);
  await seedAuth(page, user.token);

  await page.goto("/settings");
  await page.getByRole("button", { name: "アプリパスワード" }).click();
  await expect(page).toHaveURL(/\/settings\/app-passwords$/);

  await page.getByPlaceholder("用途（例: TOKIMEKI）").fill("TOKIMEKI");
  await page.getByRole("button", { name: "発行", exact: true }).click();
  const issued = page.locator("code");
  await expect(issued).toHaveText(/^[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}$/, {
    timeout: 15_000,
  });
  const password = (await issued.textContent())!;

  const session = await createSession(request, did, password);

  page.once("dialog", (dialog) => dialog.accept());
  await page
    .getByRole("listitem")
    .filter({ hasText: "TOKIMEKI" })
    .getByRole("button", { name: "無効化" })
    .click();
  await expect(page.getByText("発行済みのアプリパスワードはありません。")).toBeVisible();

  const refreshRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.refreshSession`, {
    headers: bearer(session.refreshJwt),
  });
  expect(refreshRes.status()).toBe(401);
  const loginRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.createSession`, {
    data: { identifier: did, password },
  });
  expect(loginRes.status()).toBe(401);
});

test("アプリパスワードはメインパスワードの ATP セッションで管理でき、アプリパスワードのセッションでは管理できない", async ({
  request,
}) => {
  // 全テスト共通のパスワードで createSession すると、「同じパスワードを別々のユーザーで試した
  // 数」のログイン総当たり対策に他のテストと合算で引っかかるので、このテストだけのパスワードにする。
  const mainPassword = `e2eapppw-${Date.now()}`;
  const user = await registerUserWithPasswordViaApi(request, "e2eapppwxrpc", mainPassword);
  const did = await getOwnDid(request, user.token, user.username);

  // bsky.app の設定画面と同じく、メインパスワードの ATP セッションで発行する。
  const main = await createSession(request, did, mainPassword);
  const createRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.createAppPassword`, {
    headers: bearer(main.accessJwt),
    data: { name: "client-a" },
  });
  expect(createRes.ok(), `createAppPassword失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const password = (await createRes.json()).password as string;

  // 有効なものの中で名前は一意。
  const dupRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.createAppPassword`, {
    headers: bearer(main.accessJwt),
    data: { name: "client-a" },
  });
  expect(dupRes.status()).toBe(409);

  const app = await createSession(request, did, password);
  for (const [method, data] of [
    ["com.atproto.server.createAppPassword", { name: "client-b" }],
    ["com.atproto.server.revokeAppPassword", { name: "client-a" }],
  ] as const) {
    const res = await request.post(`${BACKEND_URL}/xrpc/${method}`, { headers: bearer(app.accessJwt), data });
    expect(res.status(), `${method} がアプリパスワードで通ってしまった`).toBe(403);
  }

  const listRes = await request.get(`${BACKEND_URL}/xrpc/com.atproto.server.listAppPasswords`, {
    headers: bearer(main.accessJwt),
  });
  expect(listRes.ok()).toBeTruthy();
  expect(((await listRes.json()).passwords as { name: string }[]).map((p) => p.name)).toEqual(["client-a"]);

  const revokeRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.revokeAppPassword`, {
    headers: bearer(main.accessJwt),
    data: { name: "client-a" },
  });
  expect(revokeRes.ok()).toBeTruthy();
  const refreshRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.refreshSession`, {
    headers: bearer(app.refreshJwt),
  });
  expect(refreshRes.status()).toBe(401);

  // メインパスワードのセッションは巻き込まれない。
  const mainRefresh = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.refreshSession`, {
    headers: bearer(main.refreshJwt),
  });
  expect(mainRefresh.ok()).toBeTruthy();
});
