// アプリパスワードでログインした AT Protocol セッションには、DID 移行（PLC 操作）と
// アカウント無効化をさせない。アプリパスワードはサードパーティに渡すものなので、
// 許すと渡した相手が DID を乗っ取れる（公式 PDS と同じ制限）。

import { test, expect, type APIRequestContext } from "@playwright/test";
import { getOwnDid, registerUserViaApi } from "../fixtures/api-helpers";
import { BACKEND_URL } from "../ports.ts";

async function createSession(request: APIRequestContext, identifier: string, password: string) {
  const res = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.createSession`, {
    data: { identifier, password },
  });
  expect(res.ok(), `createSession失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).accessJwt as string;
}

test("アプリパスワードのセッションは PLC 操作・アカウント無効化を拒否され、メインパスワードのセッションは通る", async ({
  request,
}) => {
  const user = await registerUserViaApi(request, "e2eapppw");
  const did = await getOwnDid(request, user.token, user.username);

  const appPwRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.createAppPassword`, {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { name: "third-party" },
  });
  expect(appPwRes.ok(), `createAppPassword失敗: ${appPwRes.status()} ${await appPwRes.text()}`).toBeTruthy();
  const appPassword = (await appPwRes.json()).password as string;

  const appJwt = await createSession(request, did, appPassword);
  const auth = (jwt: string) => ({ Authorization: `Bearer ${jwt}` });

  // 通常の操作はアプリパスワードでもできる。
  const sessionRes = await request.get(`${BACKEND_URL}/xrpc/com.atproto.server.getSession`, {
    headers: auth(appJwt),
  });
  expect(sessionRes.ok()).toBeTruthy();

  for (const [method, data] of [
    ["com.atproto.identity.requestPlcOperationSignature", {}],
    ["com.atproto.identity.signPlcOperation", { token: "" }],
    ["com.atproto.identity.submitPlcOperation", { operation: {} }],
    ["com.atproto.server.deactivateAccount", {}],
  ] as const) {
    const res = await request.post(`${BACKEND_URL}/xrpc/${method}`, { headers: auth(appJwt), data });
    expect(res.status(), `${method} がアプリパスワードで通ってしまった`).toBe(403);
    expect((await res.json()).code).toBe("APP_PASSWORD_NOT_PERMITTED");
  }

  // メインパスワードのセッションは権限チェックを通る（E2E は SMTP 未設定なので空応答で成功する）。
  const mainJwt = await createSession(request, did, "seiranda-e2e");
  const mainRes = await request.post(`${BACKEND_URL}/xrpc/com.atproto.identity.requestPlcOperationSignature`, {
    headers: auth(mainJwt),
    data: {},
  });
  expect(mainRes.ok(), `メインパスワードで拒否された: ${mainRes.status()} ${await mainRes.text()}`).toBeTruthy();
});
