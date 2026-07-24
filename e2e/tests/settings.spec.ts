import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("設定メニューからアカウント設定画面へ遷移でき、DIDが表示される", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eset");
  await seedAuth(page, user.token);

  await page.goto("/settings");
  await expect(page.getByRole("button", { name: "アカウント" })).toBeVisible();

  await page.getByRole("button", { name: "アカウント" }).click();
  await expect(page).toHaveURL(/\/settings\/account$/);
  await expect(page.getByText(/did:/)).toBeVisible({ timeout: 15_000 });
});

test("パスワードを変更でき、新しいパスワードでログインできる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2epw");
  await seedAuth(page, user.token);

  await page.goto("/settings/account");

  // 現在のパスワードが間違っていればエラー。
  await page.getByLabel("現在のパスワード").fill("wrong-password");
  await page.getByLabel("新しいパスワード（8文字以上）").fill("new-password-123");
  await page.getByLabel("新しいパスワード（確認）").fill("new-password-123");
  await page.getByRole("button", { name: "パスワードを変更" }).click();
  await expect(page.getByText("現在のパスワードが正しくありません")).toBeVisible({ timeout: 15_000 });

  // 正しい現在のパスワードなら成功する。
  await page.getByLabel("現在のパスワード").fill("seiranda-e2e");
  await page.getByRole("button", { name: "パスワードを変更" }).click();
  await expect(page.getByText("パスワードを変更しました。")).toBeVisible({ timeout: 15_000 });

  // 新しいパスワードでログインできることを確認する。
  const loginRes = await request.post("/api/auth/login", {
    data: { identifier: user.username, password: "new-password-123" },
  });
  expect(loginRes.ok(), `新パスワードでのログイン失敗: ${loginRes.status()} ${await loginRes.text()}`).toBeTruthy();
});

test("ミュート・ブロック一覧に対象者が表示され、解除するとリストから消える", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2emb1");
  const bob = await registerUserViaApi(request, "e2emb2");

  const muteRes = await request.post("/api/mutes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(muteRes.ok()).toBeTruthy();
  const blockRes = await request.post("/api/blocks/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(blockRes.ok()).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto("/settings/mutes-blocks");

  await expect(page.getByText(`@${bob.username}`).first()).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "ミュート解除" }).click();
  await expect(page.getByText("ミュート中のユーザーはいません。")).toBeVisible({ timeout: 15_000 });

  await page.getByRole("button", { name: "ブロック中" }).click();
  await expect(page.getByText(`@${bob.username}`).first()).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "ブロック解除" }).click();
  await expect(page.getByText("ブロック中のユーザーはいません。")).toBeVisible({ timeout: 15_000 });
});

test("メールアドレス変更フォームから新アドレスを送信できる（#59、E2E環境はSMTP未設定のため送信失敗まで確認）", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2emailchg");
  await seedAuth(page, user.token);

  await page.goto("/settings/account");
  await page.getByLabel("新しいメールアドレス").fill("new-address@example.com");
  await page.getByRole("button", { name: "確認メールを送信" }).click();

  // E2E専用DBにはSMTP設定が投入されていないため、確認メール送信自体は
  // SMTP_NOT_CONFIGURED で失敗する。UI側がリクエスト〜エラー表示まで正しく
  // つながっていることを確認する（実送信を伴う成功パスはRust結合テスト側で検証済み）。
  await expect(page.getByText("メール送信機能が設定されていません。管理者にお問い合わせください")).toBeVisible({
    timeout: 15_000,
  });
});

test("アプリトークン一覧に発行済みトークンが表示され、無効化すると以後そのトークンでは認証できなくなる（#60）", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2eapptok");

  // MiAuth 認可成立を模す（Aria 等サードパーティクライアントが
  // POST /api/miauth/:session_id/authorize を叩く経路の直接呼び出し）。
  const sessionId = `e2e-session-${Date.now().toString(36)}`;
  const authorizeRes = await request.post(`/api/miauth/${sessionId}/authorize`, {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { name: "TestClient" },
  });
  expect(authorizeRes.ok(), `authorize failed: ${authorizeRes.status()} ${await authorizeRes.text()}`).toBeTruthy();

  const checkRes = await request.post("/api/miauth/check", {
    data: { session: sessionId },
  });
  expect(checkRes.ok(), `check failed: ${checkRes.status()} ${await checkRes.text()}`).toBeTruthy();
  const clientToken = (await checkRes.json()).token as string;

  // 発行直後は有効なトークンとして使える。
  const meBefore = await request.get("/api/auth/me", {
    headers: { Authorization: `Bearer ${clientToken}` },
  });
  expect(meBefore.ok()).toBeTruthy();

  await seedAuth(page, user.token);
  await page.goto("/settings/app-tokens");

  await expect(page.getByText("TestClient")).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "無効化" }).click();
  await expect(page.getByText("発行済みのアプリトークンはありません。")).toBeVisible({ timeout: 15_000 });

  // 無効化後は同じトークンでの認証が拒否される。
  const meAfter = await request.get("/api/auth/me", {
    headers: { Authorization: `Bearer ${clientToken}` },
  });
  expect(meAfter.status()).toBe(401);
});

test("設定メニューから表示設定画面へ遷移でき、言語を切り替えるとUIの表示言語が変わり保存される", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2elang");
  await seedAuth(page, user.token);

  await page.goto("/settings");
  await page.getByRole("button", { name: "表示" }).click();
  await expect(page).toHaveURL(/\/settings\/appearance$/);

  await page.getByRole("radio", { name: "English" }).check();
  await expect(page.getByText("Language preference saved.")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("heading", { name: "Appearance settings" }).or(page.getByText("Appearance settings"))).toBeVisible();

  const meRes = await request.get("/api/auth/me", {
    headers: { Authorization: `Bearer ${user.token}` },
  });
  expect(meRes.ok()).toBeTruthy();
  expect((await meRes.json()).language_preference).toBe("en");

  await page.getByRole("radio", { name: "自動", exact: false }).or(page.getByRole("radio", { name: "Automatic" })).check();
  await expect(page.getByText("言語設定を保存しました。").or(page.getByText("Language preference saved."))).toBeVisible({
    timeout: 15_000,
  });

  const meRes2 = await request.get("/api/auth/me", {
    headers: { Authorization: `Bearer ${user.token}` },
  });
  expect((await meRes2.json()).language_preference).toBeNull();
});
