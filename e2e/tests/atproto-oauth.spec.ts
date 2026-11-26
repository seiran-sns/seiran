// AT Protocol OAuth（docs/protocols.md 3節「OAuth」）。パスワードを受け取らない Bluesky
// クライアントが、PAR → 承認画面 → トークン交換 → DPoP 付きの XRPC と進めること、
// 取り消し・リフレッシュトークンの再利用で止まることを確かめる。

import { test, expect, type APIRequestContext, type Page } from "@playwright/test";
import { getOwnDid, registerUserViaApi, seedAuth, type E2eUser } from "../fixtures/api-helpers";
import {
  DpopKey,
  ISSUER,
  REDIRECT_URI,
  loopbackClientId,
  pkce,
  postWithDpop,
  xrpcWithDpop,
} from "../fixtures/atproto-oauth-client";
import { BACKEND_URL, LOCAL_DOMAIN } from "../ports.ts";

interface Tokens {
  access_token: string;
  refresh_token: string;
  scope: string;
  sub: string;
  token_type: string;
}

/** PAR から承認画面での「許可」、トークン交換までを通す。 */
async function authorize(
  page: Page,
  request: APIRequestContext,
  user: E2eUser,
  scope: string,
): Promise<{ key: DpopKey; nonce: { nonce?: string }; tokens: Tokens; clientId: string }> {
  const key = await DpopKey.generate();
  const nonce: { nonce?: string } = {};
  const clientId = loopbackClientId(scope);
  const { verifier, challenge } = pkce();
  const state = `st-${Date.now()}`;

  const par = await postWithDpop(
    request,
    key,
    "/oauth/atproto/par",
    {
      client_id: clientId,
      response_type: "code",
      code_challenge: challenge,
      code_challenge_method: "S256",
      redirect_uri: REDIRECT_URI,
      scope,
      state,
      login_hint: `${user.username}.${LOCAL_DOMAIN}`,
    },
    nonce,
  );
  expect(par.status(), `PAR失敗: ${await par.text()}`).toBe(201);
  const requestUri = (await par.json()).request_uri as string;

  const authz = await request.get(
    `${BACKEND_URL}/oauth/atproto/authorize?client_id=${encodeURIComponent(clientId)}&request_uri=${encodeURIComponent(requestUri)}`,
    { maxRedirects: 0 },
  );
  expect(authz.status()).toBe(303);
  const consentPath = authz.headers()["location"];
  expect(consentPath).toMatch(/^\/oauth-atproto\?request_uri=/);

  // 戻り先（クライアント）は存在しないので、遷移を横取りして URL だけ受け取る。
  let callbackUrl = "";
  await page.route(`${REDIRECT_URI}**`, async (route) => {
    callbackUrl = route.request().url();
    await route.fulfill({ status: 200, body: "ok" });
  });
  await seedAuth(page, user.token);
  await page.goto(consentPath);
  await expect(page.getByText("提供元: localhost")).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "許可する" }).click();
  await expect.poll(() => callbackUrl, { timeout: 15_000 }).not.toBe("");

  const cb = new URL(callbackUrl);
  expect(cb.searchParams.get("state")).toBe(state);
  expect(cb.searchParams.get("iss")).toBe(ISSUER);
  const code = cb.searchParams.get("code")!;

  const tokenRes = await postWithDpop(
    request,
    key,
    "/oauth/atproto/token",
    {
      grant_type: "authorization_code",
      client_id: clientId,
      code,
      code_verifier: verifier,
      redirect_uri: REDIRECT_URI,
    },
    nonce,
  );
  expect(tokenRes.status(), `トークン交換失敗: ${await tokenRes.text()}`).toBe(200);
  const tokens = (await tokenRes.json()) as Tokens;
  expect(tokens.token_type).toBe("DPoP");
  return { key, nonce, tokens, clientId };
}

test("メタデータから認可サーバーを見つけられる", async ({ request }) => {
  const prRes = await request.get(`${BACKEND_URL}/.well-known/oauth-protected-resource`);
  expect(prRes.ok()).toBeTruthy();
  expect((await prRes.json()).authorization_servers).toEqual([ISSUER]);

  const asRes = await request.get(`${BACKEND_URL}/.well-known/oauth-authorization-server`);
  expect(asRes.ok()).toBeTruthy();
  const meta = await asRes.json();
  expect(meta.issuer).toBe(ISSUER);
  expect(meta.pushed_authorization_request_endpoint).toBe(`${ISSUER}/oauth/atproto/par`);
  expect(meta.dpop_signing_alg_values_supported).toEqual(["ES256"]);
});

test("PAR は nonce の無い DPoP 証明に nonce を渡して送り直させる", async ({ request }) => {
  const key = await DpopKey.generate();
  const res = await request.post(`${BACKEND_URL}/oauth/atproto/par`, {
    form: { client_id: loopbackClientId("atproto") },
    headers: { DPoP: await key.proof("POST", `${ISSUER}/oauth/atproto/par`) },
  });
  expect(res.status()).toBe(400);
  expect((await res.json()).error).toBe("use_dpop_nonce");
  expect(res.headers()["dpop-nonce"]).toBeTruthy();
});

test("承認したクライアントが DPoP 付きで XRPC を使え、リフレッシュトークンの再利用でセッションが止まる", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2eatoauth");
  const did = await getOwnDid(request, user.token, user.username);
  const { key, nonce, tokens, clientId } = await authorize(
    page,
    request,
    user,
    "atproto transition:generic transition:email",
  );
  expect(tokens.sub).toBe(did);
  expect(tokens.scope).toBe("atproto transition:generic transition:email");

  const session = await xrpcWithDpop(request, key, "GET", "com.atproto.server.getSession", tokens.access_token, nonce);
  expect(session.status(), await session.text()).toBe(200);
  const sessionBody = await session.json();
  expect(sessionBody.did).toBe(did);
  expect(sessionBody.email).toBe(user.email);

  const post = await xrpcWithDpop(request, key, "POST", "com.atproto.repo.createRecord", tokens.access_token, nonce, {
    repo: did,
    collection: "app.bsky.feed.post",
    record: { $type: "app.bsky.feed.post", text: "OAuth からの投稿", createdAt: new Date().toISOString() },
  });
  expect(post.status(), await post.text()).toBe(200);

  // 同じ DPoP 証明の使い回しは拒否する。
  const path = "/xrpc/com.atproto.server.getSession";
  const proof = await key.proof("GET", `${ISSUER}${path}`, { nonce: nonce.nonce, accessToken: tokens.access_token });
  const headers = { Authorization: `DPoP ${tokens.access_token}`, DPoP: proof };
  expect((await request.get(`${BACKEND_URL}${path}`, { headers })).status()).toBe(200);
  expect((await request.get(`${BACKEND_URL}${path}`, { headers })).status()).toBe(401);

  // PLC 操作は OAuth のセッションには許さない。
  const plc = await xrpcWithDpop(
    request,
    key,
    "POST",
    "com.atproto.identity.requestPlcOperationSignature",
    tokens.access_token,
    nonce,
    {},
  );
  expect(plc.status()).toBe(403);

  // 別の鍵ではリフレッシュできない。
  const otherKey = await DpopKey.generate();
  const stolen = await postWithDpop(
    request,
    otherKey,
    "/oauth/atproto/token",
    { grant_type: "refresh_token", client_id: clientId, refresh_token: tokens.refresh_token },
    {},
  );
  expect(stolen.status()).toBe(400);

  const refreshed = await postWithDpop(
    request,
    key,
    "/oauth/atproto/token",
    { grant_type: "refresh_token", client_id: clientId, refresh_token: tokens.refresh_token },
    nonce,
  );
  expect(refreshed.status(), await refreshed.text()).toBe(200);
  const next = (await refreshed.json()) as Tokens;
  expect(next.refresh_token).not.toBe(tokens.refresh_token);

  // 使用済みのリフレッシュトークンが再び使われたら、漏えいとみなしてセッションごと止める。
  const reused = await postWithDpop(
    request,
    key,
    "/oauth/atproto/token",
    { grant_type: "refresh_token", client_id: clientId, refresh_token: tokens.refresh_token },
    nonce,
  );
  expect((await reused.json()).error).toBe("invalid_grant");
  const after = await xrpcWithDpop(request, key, "GET", "com.atproto.server.getSession", next.access_token, nonce);
  expect(after.status()).toBe(401);
});

test("transition:generic の無いセッションは書き込めず、メールアドレスも見えない", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eatoauthid");
  const did = await getOwnDid(request, user.token, user.username);
  const { key, nonce, tokens } = await authorize(page, request, user, "atproto");

  const session = await xrpcWithDpop(request, key, "GET", "com.atproto.server.getSession", tokens.access_token, nonce);
  expect(session.status()).toBe(200);
  expect((await session.json()).email).toBeUndefined();

  const post = await xrpcWithDpop(request, key, "POST", "com.atproto.repo.createRecord", tokens.access_token, nonce, {
    repo: did,
    collection: "app.bsky.feed.post",
    record: { $type: "app.bsky.feed.post", text: "書けない", createdAt: new Date().toISOString() },
  });
  expect(post.status()).toBe(403);
});

test("設定のアプリトークン画面で OAuth の連携を取り消すと、そのトークンは使えなくなる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eatoauthrv");
  const { key, nonce, tokens } = await authorize(page, request, user, "atproto transition:generic");

  await page.goto("/settings/app-tokens");
  const row = page.getByRole("listitem").filter({ hasText: "Bluesky アプリ · localhost" });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.getByRole("button", { name: "無効化" }).click();
  await expect(row).toHaveCount(0);

  const res = await xrpcWithDpop(request, key, "GET", "com.atproto.server.getSession", tokens.access_token, nonce);
  expect(res.status()).toBe(401);
});
