// 認証・ユーザー操作へのレート制限（#223）のE2Eテスト。
//
// 人手での全パターン確認は非現実的なため、各制限を実際に発動・解除させて確認する。
// `playwright.config.ts` で `workers: 1`（全テスト直列実行）のため、サイト設定
// （グローバル）を一時的に変更するテストは、必ず try/finally で元の値へ戻す。
//
// IPベースの制限（IPブロック・アカウント作成IP制限）は `ClientIp`
// (`crates/seiran-api/src/middleware/client_ip.rs`) が `x-forwarded-for` 等の
// 信頼済みヘッダーからのみIPを読み取る実装のため、テストごとに一意の
// `x-forwarded-for` を送ることで他テストと隔離できる。

import { test, expect } from "@playwright/test";
import {
  ADMIN_USERNAME,
  ADMIN_PASSWORD,
  loginViaApi,
  patchSiteSettings,
  registerUserViaApi,
  registerUserWithPasswordViaApi,
  seedAuth,
} from "../fixtures/api-helpers";

const DEFAULT_SETTINGS = {
  auth_bruteforce_window_minutes: "10",
  auth_bruteforce_max_variants: "5",
  auth_ip_block_window_minutes: "10",
  auth_ip_block_threshold: "5",
  auth_ip_block_duration_hours: "24",
  turnstile_site_key: "",
  turnstile_secret_key: "",
  post_rate_limit_window_minutes: "60",
  post_rate_limit_max_user: "30",
  post_rate_limit_max_moderator: "100",
  follow_rate_limit_window_hours: "24",
  follow_rate_limit_max_user: "100",
  follow_rate_limit_max_moderator: "300",
  list_max_count_user: "5",
  list_max_count_moderator: "30",
  list_member_max_user: "50",
  list_member_max_moderator: "300",
  search_rate_limit_window_minutes: "60",
  search_rate_limit_max_user: "10",
  search_rate_limit_max_moderator: "50",
};

let uniqueIpCounter = 0;
function uniqueIp(): string {
  uniqueIpCounter += 1;
  return `203.0.113.${uniqueIpCounter}`;
}

test.describe("ログイン/TOTPブルートフォース対策", () => {
  test("同一ユーザー名への異なるパスワード試行が種類数の閾値で拒否される", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { auth_bruteforce_max_variants: "2" });
    try {
      // 固定パスワードのregisterUserViaApiだと、他テストの同一パスワードでの
      // ログイン試行と「同一パスワードへの異なるユーザー名試行」（逆方向判定）が
      // 干渉するため、一意パスワードで隔離する。
      const user = await registerUserWithPasswordViaApi(request, "e2ebrute", `pw-${Date.now()}-a`);
      const ip = uniqueIp();
      const attempt = (password: string) =>
        request.post("/api/auth/login", {
          headers: { "x-forwarded-for": ip },
          data: { identifier: user.username, password },
        });

      const first = await attempt("wrong-password-1");
      expect(first.status()).toBe(401);
      const second = await attempt("wrong-password-2");
      expect(second.status()).toBe(401);

      // 3種類目（正しいパスワードであっても）は種類数超過で拒否される。
      const third = await attempt(user.password);
      expect(third.status()).toBe(429);
      expect((await third.json()).code).toBe("AUTH_RATE_LIMITED");
    } finally {
      await patchSiteSettings(request, admin, {
        auth_bruteforce_max_variants: DEFAULT_SETTINGS.auth_bruteforce_max_variants,
      });
    }
  });

  test("ログイン成功でブルートフォース試行の種類数カウントがリセットされる", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { auth_bruteforce_max_variants: "1" });
    try {
      const user = await registerUserWithPasswordViaApi(request, "e2ebrutereset", `pw-${Date.now()}-b`);
      const ip = uniqueIp();
      const attempt = (password: string) =>
        request.post("/api/auth/login", {
          headers: { "x-forwarded-for": ip },
          data: { identifier: user.username, password },
        });

      // 正しいパスワードで成功させる（1種類目を消費するが、成功でウィンドウがリセットされる）。
      const success = await attempt(user.password);
      expect(success.ok(), `login failed: ${success.status()} ${await success.text()}`).toBeTruthy();

      // リセットされていれば、直後の誤りパスワードはリセット後の1種類目として扱われ拒否されない。
      // （リセットされていなければ、直前の成功と合わせて2種類目になり max_variants=1 で拒否される）
      const afterSuccess = await attempt("wrong-password-after-success");
      expect(afterSuccess.status()).toBe(401);
      expect((await afterSuccess.json()).code).toBe("INVALID_CREDENTIALS");
    } finally {
      await patchSiteSettings(request, admin, {
        auth_bruteforce_max_variants: DEFAULT_SETTINGS.auth_bruteforce_max_variants,
      });
    }
  });

  test("同一パスワードへの異なるユーザー名試行も種類数の閾値で拒否される（逆方向判定）", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { auth_bruteforce_max_variants: "2" });
    try {
      const ip = uniqueIp();
      const sharedPassword = `shared-guess-${Date.now()}`;
      const attempt = (identifier: string) =>
        request.post("/api/auth/login", {
          headers: { "x-forwarded-for": ip },
          data: { identifier, password: sharedPassword },
        });

      const first = await attempt(`no-such-user-1-${Date.now()}`);
      expect(first.status()).toBe(401);
      const second = await attempt(`no-such-user-2-${Date.now()}`);
      expect(second.status()).toBe(401);
      const third = await attempt(`no-such-user-3-${Date.now()}`);
      expect(third.status()).toBe(429);
      expect((await third.json()).code).toBe("AUTH_RATE_LIMITED");
    } finally {
      await patchSiteSettings(request, admin, {
        auth_bruteforce_max_variants: DEFAULT_SETTINGS.auth_bruteforce_max_variants,
      });
    }
  });
});

test.describe("IPブロック（管理画面の一覧・解除含む）", () => {
  test("拒否がしきい値に達したIPは自動ブロックされ、管理画面で一覧・解除できる", async ({ page, request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, {
      auth_bruteforce_max_variants: "1",
      auth_ip_block_threshold: "2",
    });
    try {
      const ip = uniqueIp();
      const user = await registerUserWithPasswordViaApi(request, "e2eipblock", `pw-${Date.now()}-c`);
      const attempt = (password: string) =>
        request.post("/api/auth/login", {
          headers: { "x-forwarded-for": ip },
          data: { identifier: user.username, password },
        });

      // max_variants=1 のため、2回目の試行から毎回429（拒否）になる。
      // threshold=2 なので、2回拒否させた時点でこのIPがブロックされる。
      const r1 = await attempt("guess-1");
      expect(r1.status()).toBe(401); // 1種類目はまだ拒否されない
      const r2 = await attempt("guess-2");
      expect(r2.status()).toBe(429); // 2種類目で拒否（1回目の拒否）
      const r3 = await attempt("guess-3");
      expect(r3.status()).toBe(429); // 3種類目で拒否（2回目の拒否 → IPブロック閾値到達）

      // 以後は資格情報に関わらずIPブロックで拒否される。
      const blocked = await attempt(user.password);
      expect(blocked.status()).toBe(429);
      expect((await blocked.json()).code).toBe("IP_BLOCKED");

      // 管理画面「認証ブロック」タブに表示される。
      page.on("dialog", (dialog) => dialog.accept());
      await seedAuth(page, admin);
      await page.goto("/admin");
      await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });
      await page.getByRole("button", { name: "認証ブロック" }).click();
      const row = page.getByText(ip);
      await expect(row).toBeVisible({ timeout: 10_000 });

      // 個別解除すると一覧から消え、以後のログイン試行がIP_BLOCKEDでなくなる。
      const rowContainer = row.locator("xpath=..");
      await rowContainer.getByRole("button", { name: "解除" }).click();
      await expect(row).toHaveCount(0, { timeout: 10_000 });

      // IPブロックは解除されたが、このIPからの試行種類数自体は直近ウィンドウ内に
      // 残っているため（ブルートフォース種類数制限は別の仕組み）、ここでは
      // 「IP_BLOCKEDというコードでは拒否されなくなったこと」だけを確認する。
      const afterUnblock = await attempt(user.password);
      expect((await afterUnblock.json()).code).not.toBe("IP_BLOCKED");
    } finally {
      await patchSiteSettings(request, admin, {
        auth_bruteforce_max_variants: DEFAULT_SETTINGS.auth_bruteforce_max_variants,
        auth_ip_block_threshold: DEFAULT_SETTINGS.auth_ip_block_threshold,
      });
    }
  });
});

test.describe("Cloudflare Turnstile", () => {
  test("site key設定時はログイン・登録画面にTurnstileウィジェット領域が表示され送信ボタンが無効化される", async ({
    page,
    request,
  }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    // 実際のCloudflare検証はE2E環境から到達できないため、widget領域の出現と
    // 未トークン時の送信不可のみを確認する（ダミーのsite/secret鍵を使う）。
    await patchSiteSettings(request, admin, {
      turnstile_site_key: "1x00000000000000000000AA",
      turnstile_secret_key: "1x0000000000000000000000000000000AA",
    });
    try {
      // 実際のCloudflareスクリプトがロードされるとテスト用site keyは即座に
      // トークンを発行してしまい非決定的になるため、スクリプト自体をブロックして
      // widget領域の出現と「未トークン時は送信不可」だけを安定して確認する。
      await page.route("**/challenges.cloudflare.com/**", (route) => route.abort());

      await page.goto("/login");
      const loginButton = page.getByRole("button", { name: "ログイン", exact: true });
      await page.getByLabel("メールアドレス / ユーザーネーム").fill("someone");
      await page.getByLabel("パスワード").fill("some-password");
      // Turnstileトークン未取得のため送信ボタンが無効。
      await expect(loginButton).toBeDisabled({ timeout: 10_000 });

      // E2E環境はrequireEmailVerification=falseのため直接登録フローが表示される。
      await page.goto("/register");
      await expect(page.getByRole("button", { name: "登録する" })).toBeDisabled({
        timeout: 10_000,
      });
    } finally {
      await patchSiteSettings(request, admin, {
        turnstile_site_key: DEFAULT_SETTINGS.turnstile_site_key,
        turnstile_secret_key: DEFAULT_SETTINGS.turnstile_secret_key,
      });
    }
  });
});

test.describe("ロール別レート制限", () => {
  test("投稿数が1時間あたりの上限に達すると拒否される", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { post_rate_limit_max_user: "2" });
    try {
      const user = await registerUserViaApi(request, "e2epostlimit");
      const post = (text: string) =>
        request.post("/api/notes/create", {
          headers: { Authorization: `Bearer ${user.token}` },
          data: { text, visibility: "public" },
        });

      const r1 = await post("投稿1");
      expect(r1.ok(), `post 1 failed: ${r1.status()}`).toBeTruthy();
      const r2 = await post("投稿2");
      expect(r2.ok(), `post 2 failed: ${r2.status()}`).toBeTruthy();
      const r3 = await post("投稿3");
      expect(r3.status()).toBe(429);
      expect((await r3.json()).code).toBe("POST_RATE_LIMITED");
    } finally {
      await patchSiteSettings(request, admin, {
        post_rate_limit_max_user: DEFAULT_SETTINGS.post_rate_limit_max_user,
      });
    }
  });

  test("新規フォロー数が24時間あたりの上限に達すると拒否される", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { follow_rate_limit_max_user: "2" });
    try {
      const user = await registerUserViaApi(request, "e2efollowlimit");
      const targets = await Promise.all([
        registerUserViaApi(request, "e2efollowtarget1"),
        registerUserViaApi(request, "e2efollowtarget2"),
        registerUserViaApi(request, "e2efollowtarget3"),
      ]);
      const follow = (target: string) =>
        request.post("/api/follows/create", {
          headers: { Authorization: `Bearer ${user.token}` },
          data: { target },
        });

      const r1 = await follow(targets[0].username);
      expect(r1.ok(), `follow 1 failed: ${r1.status()}`).toBeTruthy();
      const r2 = await follow(targets[1].username);
      expect(r2.ok(), `follow 2 failed: ${r2.status()}`).toBeTruthy();
      const r3 = await follow(targets[2].username);
      expect(r3.status()).toBe(429);
      expect((await r3.json()).code).toBe("FOLLOW_RATE_LIMITED");
    } finally {
      await patchSiteSettings(request, admin, {
        follow_rate_limit_max_user: DEFAULT_SETTINGS.follow_rate_limit_max_user,
      });
    }
  });

  test("リスト作成数の上限に達すると拒否される", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { list_max_count_user: "1" });
    try {
      const user = await registerUserViaApi(request, "e2elistlimit");
      const createList = (name: string) =>
        request.post("/api/lists", {
          headers: { Authorization: `Bearer ${user.token}` },
          data: { name, is_public: false },
        });

      const r1 = await createList("リスト1");
      expect(r1.ok(), `create list 1 failed: ${r1.status()}`).toBeTruthy();
      const r2 = await createList("リスト2");
      expect(r2.status()).toBe(409);
      expect((await r2.json()).code).toBe("LIST_LIMIT_EXCEEDED");
    } finally {
      await patchSiteSettings(request, admin, {
        list_max_count_user: DEFAULT_SETTINGS.list_max_count_user,
      });
    }
  });

  test("リストメンバー数の上限に達すると拒否される", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { list_member_max_user: "1" });
    try {
      const user = await registerUserViaApi(request, "e2elistmemberlimit");
      const members = await Promise.all([
        registerUserViaApi(request, "e2elistmember1"),
        registerUserViaApi(request, "e2elistmember2"),
      ]);

      const createRes = await request.post("/api/lists", {
        headers: { Authorization: `Bearer ${user.token}` },
        data: { name: "メンバー制限テスト", is_public: false },
      });
      expect(createRes.ok(), `create list failed: ${createRes.status()}`).toBeTruthy();
      const list = await createRes.json();

      const addMember = (target: string) =>
        request.post(`/api/lists/${list.id}/members`, {
          headers: { Authorization: `Bearer ${user.token}` },
          data: { target },
        });

      const r1 = await addMember(members[0].username);
      expect(r1.ok(), `add member 1 failed: ${r1.status()}`).toBeTruthy();
      const r2 = await addMember(members[1].username);
      expect(r2.status()).toBe(409);
      expect((await r2.json()).code).toBe("LIST_MEMBER_LIMIT_EXCEEDED");
    } finally {
      await patchSiteSettings(request, admin, {
        list_member_max_user: DEFAULT_SETTINGS.list_member_max_user,
      });
    }
  });

  test("検索回数が1時間あたりの上限に達すると拒否される（スクロール取得は回数に含まない）", async ({
    request,
  }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { search_rate_limit_max_user: "2" });
    try {
      const user = await registerUserViaApi(request, "e2esearchlimit");
      const search = (params: string) =>
        request.get(`/api/notes/search?${params}`, {
          headers: { Authorization: `Bearer ${user.token}` },
        });

      const r1 = await search("q=hello");
      expect(r1.ok(), `search 1 failed: ${r1.status()}`).toBeTruthy();
      const r2 = await search("q=hello");
      expect(r2.ok(), `search 2 failed: ${r2.status()}`).toBeTruthy();
      const r3 = await search("q=hello");
      expect(r3.status()).toBe(429);
      expect((await r3.json()).code).toBe("SEARCH_RATE_LIMITED");

      // ページング（until_id指定）は初回検索としてカウントされないため拒否されない。
      const r4 = await search("q=hello&until_id=1");
      expect(r4.ok(), `paged search failed: ${r4.status()} ${await r4.text()}`).toBeTruthy();
    } finally {
      await patchSiteSettings(request, admin, {
        search_rate_limit_max_user: DEFAULT_SETTINGS.search_rate_limit_max_user,
      });
    }
  });

  test("adminロールはロール別レート制限の対象外", async ({ request }) => {
    const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await patchSiteSettings(request, admin, { post_rate_limit_max_user: "1" });
    try {
      const post = (text: string) =>
        request.post("/api/notes/create", {
          headers: { Authorization: `Bearer ${admin}` },
          data: { text, visibility: "public" },
        });
      const r1 = await post(`admin投稿1 ${Date.now()}`);
      expect(r1.ok(), `post 1 failed: ${r1.status()}`).toBeTruthy();
      const r2 = await post(`admin投稿2 ${Date.now()}`);
      expect(r2.ok(), `post 2 failed（adminは無制限のはず）: ${r2.status()}`).toBeTruthy();
    } finally {
      await patchSiteSettings(request, admin, {
        post_rate_limit_max_user: DEFAULT_SETTINGS.post_rate_limit_max_user,
      });
    }
  });
});
