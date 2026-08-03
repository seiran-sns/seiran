import { test, expect } from "@playwright/test";
import { loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// global-setup.ts が作成する初期管理者アカウント（role=admin）。
const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e";

test("管理者以外が/adminにアクセスするとホームへリダイレクトされる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eadminguard");
  await seedAuth(page, user.token);

  await page.goto("/admin");

  await expect(page).toHaveURL(/\/$/, { timeout: 10_000 });
});

test("管理者はサイト設定を変更でき、リロード後も反映されている", async ({ page, request }) => {
  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  await seedAuth(page, adminToken);

  await page.goto("/admin");
  // 既定タブ（ユーザー管理）が描画されるまで待ってから「サイト設定」タブへ切り替える。
  await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });
  await page.getByRole("button", { name: "サイト設定" }).click();

  const newName = `seiran-e2e-${Date.now()}`;
  const mediaProxyUrl = "http://localhost:3100";
  const nameInput = page.getByLabel("サイト名称");
  await nameInput.fill(newName);
  await page.getByLabel("外部メディアプロキシURL").fill(mediaProxyUrl);
  await page.getByRole("button", { name: "保存" }).click();

  await expect(page.getByText("保存しました。")).toBeVisible({ timeout: 10_000 });

  await page.reload();
  await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });
  await page.getByRole("button", { name: "サイト設定" }).click();
  await expect(page.getByLabel("サイト名称")).toHaveValue(newName, { timeout: 10_000 });
  await expect(page.getByLabel("外部メディアプロキシURL")).toHaveValue(mediaProxyUrl);
});

test("管理者はユーザーを凍結・凍結解除できる", async ({ page, request }) => {
  const target = await registerUserViaApi(request, "e2eadmintarget");
  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  await seedAuth(page, adminToken);

  await page.goto("/admin");
  await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });

  // ユーザー一覧はid昇順・1ページ30件のカーソルページネーション（UserManagement.tsx）
  // のため、他specの実行順序次第でtargetが初期ページに載らないことがある。
  // 絞り込みボックスで確実に対象だけを表示させてから探す。
  // 行構造: subText(@username · email) → grow → row の2階層上に、凍結ボタン・
  // ロール選択を含む行全体(row)がある。subTextはemail併記のため exact match しない。
  await page.getByPlaceholder("表示名・ユーザー名・メールアドレスで絞り込み").fill(target.username);
  const usernameCell = page.getByText(`@${target.username}`);
  await expect(usernameCell).toBeVisible({ timeout: 10_000 });
  const row = usernameCell.locator("xpath=../..");

  await row.getByRole("button", { name: "凍結", exact: true }).click();
  await expect(row.getByText("凍結中")).toBeVisible({ timeout: 10_000 });

  await row.getByRole("button", { name: "凍結解除" }).click();
  await expect(row.getByText("凍結中")).toHaveCount(0, { timeout: 10_000 });
});

test("ユーザー管理にTOTP状態とパスキー数が表示され、TOTP強制解除APIは管理者だけが使える", async ({ page, request }) => {
  const target = await registerUserViaApi(request, "e2eadminauthstatus");
  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

  const denied = await request.post(`/api/admin/users/${target.userId}/totp/disable`, {
    headers: { Authorization: `Bearer ${target.token}` },
  });
  expect(denied.status()).toBe(403);

  const disabled = await request.post(`/api/admin/users/${target.userId}/totp/disable`, {
    headers: { Authorization: `Bearer ${adminToken}` },
  });
  expect(disabled.status()).toBe(204);

  await seedAuth(page, adminToken);
  await page.goto("/admin");
  await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });
  // 上記凍結テストと同様、ユーザー一覧のページネーションにより初期ページに
  // targetが載らないことがあるため絞り込みボックスで確実に表示させる。
  await page.getByPlaceholder("表示名・ユーザー名・メールアドレスで絞り込み").fill(target.username);
  const usernameCell = page.getByText(`@${target.username}`);
  await expect(usernameCell).toBeVisible({ timeout: 10_000 });
  const row = usernameCell.locator("xpath=../..");
  await expect(row.getByText("TOTP: 無効")).toBeVisible();
  await expect(row.getByText("パスキー: 0件")).toBeVisible();
});

test("Fedi受信でリモート絵文字をカタログ化し、管理者だけが検索できる（#73）", async ({ request }) => {
  const user = await registerUserViaApi(request, "e2eremoteemoji");
  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const fedi = await startStubFediServer();

  try {
    const shortcode = `remote_${Date.now().toString(36)}`;
    await fedi.sendCreateNote(SEIRAN_BASE_URL, user.username, `:${shortcode}:`, { emojiShortcode: shortcode });

    const listUrl = `/api/admin/emojis/remote?keyword=${encodeURIComponent(`alias_${shortcode}`)}`;
    const denied = await request.get(listUrl, {
      headers: { Authorization: `Bearer ${user.token}` },
    });
    expect(denied.status()).toBe(403);

    await expect
      .poll(
        async () => {
          const res = await request.get(listUrl, {
            headers: { Authorization: `Bearer ${adminToken}` },
          });
          if (!res.ok()) return [];
          return (await res.json()) as {
            shortcode: string;
            domain: string;
            imageUrl: string;
            tags: string[];
            license: string | null;
          }[];
        },
        { timeout: 15_000 },
      )
      .toEqual([
        expect.objectContaining({
          shortcode,
          domain: new URL(fedi.url).host,
          imageUrl: `${fedi.url}/emoji/${shortcode}.png`,
          tags: [`alias_${shortcode}`],
          license: "CC BY 4.0 E2E",
        }),
      ]);
  } finally {
    await fedi.close();
  }
});
