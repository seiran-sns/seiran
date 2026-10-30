import { test, expect, APIRequestContext } from "@playwright/test";
import {
  ADMIN_PASSWORD,
  ADMIN_USERNAME,
  E2eUser,
  loginViaApi,
  patchSiteSettings,
  registerUserViaApi,
  seedAuth,
} from "../fixtures/api-helpers";

async function setRole(request: APIRequestContext, user: E2eUser, role: string) {
  const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const res = await request.post(`/api/admin/users/${user.userId}/role`, {
    headers: { Authorization: `Bearer ${admin}` },
    data: { role },
  });
  expect(res.ok(), `ロール変更失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
}

async function follow(request: APIRequestContext, follower: E2eUser, target: E2eUser) {
  const res = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${follower.token}` },
    data: { target: target.username },
  });
  expect(res.ok(), `フォロー失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
}

async function createNote(request: APIRequestContext, user: E2eUser, data: Record<string, unknown>) {
  return request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { deliver_to_fedi: false, deliver_to_bsky: false, ...data },
  });
}

async function createdId(res: Awaited<ReturnType<typeof createNote>>): Promise<string> {
  expect(res.ok(), `投稿失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).id as string;
}

async function expectRestricted(res: Awaited<ReturnType<typeof createNote>>) {
  expect(res.status(), await res.text()).toBe(403);
  expect((await res.json()).code).toBe("INTERACTION_RESTRICTED");
}

test("silence はパブリック投稿がひかえめに読み替えられ、投稿フォームのパブリックが押せない", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2esilence");
  await setRole(request, user, "silence");

  const res = await createNote(request, user, { text: `サイレンス投稿 ${Date.now()}`, visibility: "public" });
  expect(res.ok(), `投稿失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  expect((await res.json()).visibility).toBe("unlisted");

  await seedAuth(page, user.token);
  await page.goto("/");
  await page.getByPlaceholder("いまどうしてる？").fill("サイレンス中");
  await expect(page.getByRole("button", { name: "ひかえめ" })).toBeEnabled({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "投稿", exact: true })).toBeDisabled();
});

test("quarantine はフォロワー以外へ返信・引用・メンション・リアクション・DMできないが、リポストはできる", async ({
  request,
}) => {
  const restricted = await registerUserViaApi(request, "e2equar");
  const stranger = await registerUserViaApi(request, "e2equarstr");
  const follower = await registerUserViaApi(request, "e2equarfol");
  await follow(request, follower, restricted);
  // 制限ユーザーがフォローしているだけの相手は対象外（相手からフォローされていることが条件）
  await follow(request, restricted, stranger);
  await setRole(request, restricted, "quarantine");

  const strangerNote = await createdId(await createNote(request, stranger, { text: `他人の投稿 ${Date.now()}` }));
  const followerNote = await createdId(await createNote(request, follower, { text: `フォロワーの投稿 ${Date.now()}` }));

  await expectRestricted(await createNote(request, restricted, { text: "返信", reply_to_id: strangerNote }));
  await expectRestricted(await createNote(request, restricted, { text: "引用", quote_of_id: strangerNote }));
  await expectRestricted(await createNote(request, restricted, { text: `@${stranger.username} こんにちは` }));
  await expectRestricted(
    await createNote(request, restricted, {
      text: "DM",
      visibility: "direct",
      recipient_actor_ids: [stranger.actorId],
    }),
  );
  await expectRestricted(
    await request.post(`/api/notes/${strangerNote}/reactions`, {
      headers: { Authorization: `Bearer ${restricted.token}` },
      data: { content: "🎉" },
    }),
  );

  const repost = await createNote(request, restricted, { renote_id: strangerNote });
  expect(repost.ok(), `リポスト失敗: ${repost.status()} ${await repost.text()}`).toBeTruthy();

  const reply = await createNote(request, restricted, {
    text: `@${follower.username} 返信`,
    reply_to_id: followerNote,
    visibility: "public",
  });
  expect(reply.ok(), `フォロワーへの返信失敗: ${reply.status()} ${await reply.text()}`).toBeTruthy();
  expect((await reply.json()).visibility).toBe("unlisted");
  const react = await request.post(`/api/notes/${followerNote}/reactions`, {
    headers: { Authorization: `Bearer ${restricted.token}` },
    data: { content: "🎉" },
  });
  expect(react.ok(), `フォロワーへのリアクション失敗: ${react.status()} ${await react.text()}`).toBeTruthy();

  // 調停者（admin / moderator）にはフォロワーでなくても話しかけられる
  const toAdmin = await createNote(request, restricted, { text: `@${ADMIN_USERNAME} 異議があります` });
  expect(toAdmin.ok(), `管理者へのメンション失敗: ${toAdmin.status()} ${await toAdmin.text()}`).toBeTruthy();
});

test("quarantine の新規フォロー数は独自の上限で制限される", async ({ request }) => {
  const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const restricted = await registerUserViaApi(request, "e2equarfl");
  const targets = [
    await registerUserViaApi(request, "e2equarflt"),
    await registerUserViaApi(request, "e2equarflt"),
    await registerUserViaApi(request, "e2equarflt"),
  ];
  await setRole(request, restricted, "quarantine");
  await patchSiteSettings(request, admin, { follow_rate_limit_max_quarantine: "2" });
  try {
    await follow(request, restricted, targets[0]);
    await follow(request, restricted, targets[1]);
    const third = await request.post("/api/follows/create", {
      headers: { Authorization: `Bearer ${restricted.token}` },
      data: { target: targets[2].username },
    });
    expect(third.status(), await third.text()).toBe(429);
  } finally {
    await patchSiteSettings(request, admin, { follow_rate_limit_max_quarantine: "10" });
  }
});

test("管理画面のレート制限タブはロール×項目のマトリクスで編集できる", async ({ page, request }) => {
  const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  await seedAuth(page, admin);
  await page.goto("/admin/rate-limits");
  await expect(page.getByRole("columnheader", { name: "quarantine" })).toBeVisible({ timeout: 15_000 });

  const cell = page.getByRole("spinbutton", { name: "新規フォロー数 (quarantine)" });
  await expect(cell).toHaveValue("10");
  // 独自の値を持たないロールは入力欄を出さない
  await expect(page.getByRole("spinbutton", { name: "新規フォロー数 (silence)" })).toHaveCount(0);

  try {
    await cell.fill("7");
    await page.getByRole("button", { name: "保存", exact: true }).click();
    await expect(page.getByText("保存しました。")).toBeVisible({ timeout: 10_000 });
    const res = await request.get("/api/admin/site-settings", { headers: { Authorization: `Bearer ${admin}` } });
    expect((await res.json()).follow_rate_limit_max_quarantine).toBe("7");
  } finally {
    await patchSiteSettings(request, admin, { follow_rate_limit_max_quarantine: "10" });
  }
});
