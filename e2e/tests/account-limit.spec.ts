// ユーザー数上限（site_settings.max_user_accounts）のE2Eテスト。
// 上限は全specで共有されるグローバル設定のため、terms-of-service.spec.tsと同じく排他テールで
// 実行し、必ずafterEachで無制限（0、globalSetupが設定する値）へ戻す。
// 上限を1にすれば、初期管理者だけで常に「到達済み」になる。

import { test, expect } from "@playwright/test";
import { ADMIN_USERNAME, ADMIN_PASSWORD, loginViaApi, patchSiteSettings } from "../fixtures/api-helpers";

test.describe("ユーザー数上限に到達している間", () => {
  test.beforeEach(async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { max_user_accounts: "1" });
  });
  test.afterEach(async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { max_user_accounts: "0" });
  });

  test("登録API・メール確認API・転入APIはACCOUNT_LIMIT_REACHEDで拒否される", async ({ request }) => {
    const suffix = Date.now().toString(36);
    const register = await request.post("/api/auth/register", {
      data: { username: `e2elimit${suffix}`, password: "seiranda-e2e", email: `e2e-limit-${suffix}@example.com` },
    });
    expect(register.status()).toBe(403);
    expect((await register.json()).code).toBe("ACCOUNT_LIMIT_REACHED");

    const verify = await request.post("/api/auth/verify-email", {
      data: { email: `e2e-limit-verify-${suffix}@example.com` },
    });
    expect(verify.status()).toBe(403);
    expect((await verify.json()).code).toBe("ACCOUNT_LIMIT_REACHED");

    const migrate = await request.post("/api/migration/start", {
      data: {
        source_handle: "example.invalid",
        source_password: "x",
        new_username: `e2elimitmig${suffix}`,
        new_password: "seiranda-e2e",
      },
    });
    expect(migrate.status()).toBe(403);
    expect((await migrate.json()).code).toBe("ACCOUNT_LIMIT_REACHED");
  });

  test("metaが登録不可を示し、登録画面にエラーメッセージが表示される", async ({ page, request }) => {
    const meta = await (await request.post("/api/meta")).json();
    expect(meta.accountLimitReached).toBe(true);
    expect(meta.disableRegistration).toBe(true);

    await page.goto("/register");
    await expect(page.getByText("ユーザー数が上限に達している")).toBeVisible();
    await expect(page.getByLabel("ユーザー名")).toHaveCount(0);
  });
});

test("受付停止スイッチがONの間は登録APIがREGISTRATION_CLOSEDで拒否される", async ({ request, page }) => {
  const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  await patchSiteSettings(request, admin, { registration_closed: "true" });
  try {
    const suffix = Date.now().toString(36);
    const res = await request.post("/api/auth/register", {
      data: { username: `e2eclosed${suffix}`, password: "seiranda-e2e", email: `e2e-closed-${suffix}@example.com` },
    });
    expect(res.status()).toBe(403);
    expect((await res.json()).code).toBe("REGISTRATION_CLOSED");

    await page.goto("/register");
    await expect(page.getByText("受付を停止しています")).toBeVisible();
  } finally {
    await patchSiteSettings(request, admin, { registration_closed: "false" });
  }
});

test("上限を0（無制限）にすると登録できる", async ({ request }) => {
  const suffix = Date.now().toString(36);
  const res = await request.post("/api/auth/register", {
    data: { username: `e2eunl${suffix}`, password: "seiranda-e2e", email: `e2e-unl-${suffix}@example.com` },
  });
  expect(res.ok(), `register failed: ${res.status()} ${await res.text()}`).toBeTruthy();
});
