// 利用規約同意チェックボックス（新規登録・Bluesky転入共通、`TermsOfServiceField`）のE2Eテスト。
// `patchSiteSettings`でサイト設定（グローバル）を書き換えるため、rate-limit.spec.tsと同じく
// 必ずtry/finallyで元の値（空文字列＝チェックボックス非表示）へ戻す。

import { test, expect } from "@playwright/test";
import { ADMIN_USERNAME, ADMIN_PASSWORD, loginViaApi, patchSiteSettings } from "../fixtures/api-helpers";

const TOS_TEXT = "<b>テスト利用規約</b>\n第1条 これはE2Eテスト用の文面です";

test("利用規約テキスト未設定時はサインアップ画面にチェックボックスが表示されない", async ({ page }) => {
  await page.goto("/register");
  await expect(page.getByRole("checkbox")).toHaveCount(0);
});

test.describe("利用規約テキスト設定時", () => {
  test.beforeEach(async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { terms_of_service_text: TOS_TEXT });
  });
  test.afterEach(async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { terms_of_service_text: "" });
  });

  test("チェックボックス・ダイアログが表示され、同意しないと送信できない", async ({ page }) => {
    const suffix = Date.now().toString(36);
    await page.goto("/register");
    await page.getByLabel("メールアドレス").fill(`e2e-tos-${suffix}@example.com`);
    await page.getByLabel("ユーザー名").fill(`e2etos${suffix}`);
    await page.getByLabel("パスワード（8文字以上）").fill("seiranda-e2e");

    const checkbox = page.getByRole("checkbox", { name: "利用規約に同意します" });
    await expect(checkbox).toBeVisible();
    await expect(checkbox).not.toBeChecked();

    await page.getByRole("button", { name: "利用規約を読む" }).click();
    await expect(page.getByText("これはE2Eテスト用の文面です")).toBeVisible();
    await page.getByRole("button", { name: "閉じる" }).click();

    // 未同意のまま送信 → ネイティブのrequiredバリデーションで送信自体が止まり、画面遷移しない
    // （migrate-register.spec.tsの「必須項目未入力では送信できない」と同じ検証パターン）。
    await page.getByRole("button", { name: "登録する" }).click();
    await expect(page).toHaveURL(/\/register$/);

    // 同意して送信 → 登録成功してトップへ遷移する
    await checkbox.check();
    await page.getByRole("button", { name: "登録する" }).click();
    await expect(page).toHaveURL("/");
  });

  test("同意せず登録APIを直接叩くとTOS_AGREEMENT_REQUIREDで拒否される", async ({ request }) => {
    const suffix = Date.now().toString(36);
    const res = await request.post("/api/auth/register", {
      data: {
        username: `e2etosapi${suffix}`,
        password: "seiranda-e2e",
        email: `e2e-tos-api-${suffix}@example.com`,
      },
    });
    expect(res.status()).toBe(400);
    const body = await res.json();
    expect(body.code).toBe("TOS_AGREEMENT_REQUIRED");
  });

  test("同意して登録APIを直接叩くと成功する", async ({ request }) => {
    const suffix = Date.now().toString(36);
    const res = await request.post("/api/auth/register", {
      data: {
        username: `e2etosapiok${suffix}`,
        password: "seiranda-e2e",
        email: `e2e-tos-api-ok-${suffix}@example.com`,
        agree_tos: true,
      },
    });
    expect(res.ok(), `register failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  });
});
