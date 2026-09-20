import { test, expect } from "@playwright/test";
import { loginViaApi, registerUserViaApi } from "../fixtures/api-helpers";
import { startStubS3Server } from "../fixtures/stub-s3-server";

const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e";

test("Misskey互換API: endpointsでemojisを検出して絵文字一覧を取得できる（#145）", async ({
  request,
}) => {
  const endpointsRes = await request.post("/api/endpoints", { data: {} });
  expect(endpointsRes.ok(), await endpointsRes.text()).toBeTruthy();
  expect(await endpointsRes.json()).toContain("emojis");

  const emojisRes = await request.post("/api/emojis", { data: {} });
  expect(emojisRes.ok(), await emojisRes.text()).toBeTruthy();
  expect(await emojisRes.json()).toEqual(
    expect.objectContaining({ emojis: expect.any(Array) }),
  );
});

// 1x1 透明PNG。
const MINIMAL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  "base64",
);

test("Misskey互換API: Aria形式のローカルカスタム絵文字でリアクションできる（#145）", async ({
  request,
}) => {
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  let adminToken: string | null = null;
  try {
    const alice = await registerUserViaApi(request, "e2emkreacta");
    const bob = await registerUserViaApi(request, "e2emkreactb");
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-misskey-reaction-${Date.now()}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), await providerRes.text()).toBeTruthy();
    providerId = (await providerRes.json()).id;

    const uploadRes = await request.post("/api/drive/files/create", {
      headers: { Authorization: `Bearer ${bob.token}` },
      multipart: {
        file: {
          name: "emoji.png",
          mimeType: "image/png",
          buffer: MINIMAL_PNG,
        },
        media_type: "emoji",
      },
    });
    expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
    const uploaded = await uploadRes.json();

    const shortcode = `mkreact${Date.now().toString(36)}`;
    const emojiRes = await request.post("/api/admin/emojis", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: { shortcode, media_file_id: uploaded.id },
    });
    expect(emojiRes.ok(), await emojiRes.text()).toBeTruthy();

    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { text: `Ariaリアクションテスト ${Date.now()}` },
    });
    expect(createRes.ok(), await createRes.text()).toBeTruthy();
    const note = await createRes.json();

    const reactRes = await request.post("/api/notes/reactions/create", {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { noteId: note.id, reaction: `:${shortcode}@.:` },
    });
    expect(
      reactRes.ok(),
      `Aria形式リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`,
    ).toBeTruthy();

    const showRes = await request.post("/api/notes/show", {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { noteId: note.id },
    });
    expect(showRes.ok(), await showRes.text()).toBeTruthy();
    const shown = await showRes.json();
    // 本家Misskey準拠でローカルカスタム絵文字は `@.` サフィックス付きで保存・返却される。
    expect(shown.reactions[`:${shortcode}@.:`]).toBe(1);
    expect(shown.myReaction).toBe(`:${shortcode}@.:`);
    expect(shown.reactionEmojis[`${shortcode}@.`]).toBeTruthy();
  } finally {
    if (providerId && adminToken) {
      await request.patch(`/api/admin/storage-providers/${providerId}`, {
        headers: { Authorization: `Bearer ${adminToken}` },
        data: { is_active: false },
      });
    }
    await s3.close();
  }
});

test("Misskey互換API: リポストのnotes/showでrenoteに元ノート本体が埋め込まれる（#74）", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emkrenotea");
  const bob = await registerUserViaApi(request, "e2emkrenoteb");

  const originalText = `renote元ポスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: originalText },
  });
  expect(createRes.ok(), `元投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const original = await createRes.json();

  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { renote_id: original.id },
  });
  expect(repostRes.ok(), `リポスト作成失敗: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();
  const repost = await repostRes.json();

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { noteId: repost.id },
  });
  expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.renoteId).toBe(String(original.id));
  expect(shown.renote, "renote本体がnullのまま（削除されたノート表示の原因）").not.toBeNull();
  expect(shown.renote.id).toBe(String(original.id));
  expect(shown.renote.text).toBe(originalText);
  expect(shown.renote.user.username).toBe(alice.username);
});

test("Misskey互換API: 引用ポストへのリポストでnotes/showの孫階層（renote.renote）まで埋め込まれる（#251）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkgrandca");
  const bob = await registerUserViaApi(request, "e2emkgrandcb");
  const carol = await registerUserViaApi(request, "e2emkgrandcc");

  const quotedText = `孫階層テスト元ポスト ${Date.now()}`;
  const quotedRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: quotedText },
  });
  expect(quotedRes.ok(), `引用元投稿作成失敗: ${quotedRes.status()} ${await quotedRes.text()}`).toBeTruthy();
  const quoted = await quotedRes.json();

  const quoteRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { text: "引用テスト", quote_of_id: quoted.id },
  });
  expect(quoteRes.ok(), `引用投稿作成失敗: ${quoteRes.status()} ${await quoteRes.text()}`).toBeTruthy();
  const quotePost = await quoteRes.json();

  // 引用ポストをさらにプレーンリポスト（ブースト）する。misskey/convert.rsの
  // embed_referenced_notesが1階層しか埋め込まないと、このリポストのnotes/showで
  // renote.renote（＝quotedへの参照）がrenoteIdありrenoteなしになり、Aria側で
  // 「削除されたノート」誤表示を招く（#251で修正）。
  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${carol.token}` },
    data: { renote_id: quotePost.id },
  });
  expect(repostRes.ok(), `リポスト作成失敗: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();
  const repost = await repostRes.json();

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${carol.token}` },
    data: { noteId: repost.id },
  });
  expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.renoteId).toBe(String(quotePost.id));
  expect(shown.renote, "renote(孫の親)本体がnull").not.toBeNull();
  expect(shown.renote.renoteId).toBe(String(quoted.id));
  expect(
    shown.renote.renote,
    "renote.renote（孫階層）がnullのまま（削除されたノート表示の原因、#251）",
  ).not.toBeNull();
  expect(shown.renote.renote.id).toBe(String(quoted.id));
  expect(shown.renote.renote.text).toBe(quotedText);
});

test("Misskey互換API: notes/showでreplyに返信先ノート本体が埋め込まれる（Aria非互換修正）", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emkreplya");
  const bob = await registerUserViaApi(request, "e2emkreplyb");

  const originalText = `返信先ポスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: originalText },
  });
  expect(createRes.ok(), `元投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const original = await createRes.json();

  const replyRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { text: "返信本文", reply_to_id: original.id },
  });
  expect(replyRes.ok(), `返信作成失敗: ${replyRes.status()} ${await replyRes.text()}`).toBeTruthy();
  const reply = await replyRes.json();

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { noteId: reply.id },
  });
  expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.replyId).toBe(String(original.id));
  expect(shown.reply, "reply本体がnullのまま（削除されたノート表示の原因）").not.toBeNull();
  expect(shown.reply.id).toBe(String(original.id));
  expect(shown.reply.text).toBe(originalText);
  expect(shown.reply.user.username).toBe(alice.username);
});

test("Misskey互換API: notes/showでCW付き投稿のcwが反映される（Aria非互換修正）", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emkcwa");
  const cwText = `閲覧注意 ${Date.now()}`;

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: "本文", content_warning: cwText },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { noteId: created.id },
  });
  expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.cw).toBe(cwText);
});

test("Misskey互換API: notes/showでアンケート付き投稿のpollが反映される（Aria非互換修正）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkpolla");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: {
      text: "好きな色は？",
      poll: { choices: ["赤", "青"], expiresInSeconds: 3600 },
    },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  const voteRes = await request.post(`/api/notes/${created.id}/poll-vote`, {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { optionIndexes: [0] },
  });
  expect(voteRes.ok(), `投票失敗: ${voteRes.status()} ${await voteRes.text()}`).toBeTruthy();

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { noteId: created.id },
  });
  expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.poll).toBeTruthy();
  expect(shown.poll.multiple).toBe(false);
  expect(shown.poll.choices).toEqual([
    expect.objectContaining({ text: "赤", votes: 1, isVoted: true }),
    expect.objectContaining({ text: "青", votes: 0, isVoted: false }),
  ]);
});

test("Misskey互換API: notes/polls/voteでアンケートに投票できる（MisskeyNotesPolls.vote、#252続き）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkpollvote");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: {
      text: "好きな季節は？",
      poll: { choices: ["夏", "冬"], expiresInSeconds: 3600 },
    },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  const voteRes = await request.post("/api/notes/polls/vote", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { noteId: created.id, choice: 1 },
  });
  expect(voteRes.ok(), `投票失敗: ${voteRes.status()} ${await voteRes.text()}`).toBeTruthy();
  expect(voteRes.status()).toBe(204);

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { noteId: created.id },
  });
  expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.poll.choices).toEqual([
    expect.objectContaining({ text: "夏", votes: 0, isVoted: false }),
    expect.objectContaining({ text: "冬", votes: 1, isVoted: true }),
  ]);

  // 二重投票は409（既存カスタムAPIのALREADY_VOTED判定をそのまま透過する）。
  const secondVoteRes = await request.post("/api/notes/polls/vote", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { noteId: created.id, choice: 0 },
  });
  expect(secondVoteRes.status()).toBe(409);
});

test("Misskey互換API: metaのmediaProxyUrlが未設定時に自インスタンスの/proxyへフォールバックする（Aria非互換修正）", async ({
  request,
}) => {
  const metaRes = await request.post("/api/meta", { data: {} });
  expect(metaRes.ok(), await metaRes.text()).toBeTruthy();
  const meta = await metaRes.json();

  expect(meta.mediaProxyUrl, "mediaProxyUrlが空だとAriaのURL組み立てが壊れる").toBeTruthy();
  expect(meta.mediaProxyUrl).toMatch(/\/proxy$/);
});

test("Misskey互換API: users/showのfollowersVisibility/followingVisibilityが常にpublic（#74）", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emkvisa");
  const bob = await registerUserViaApi(request, "e2emkvisb");

  const showRes = await request.post("/api/users/show", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { userId: alice.actorId },
  });
  expect(showRes.ok(), `users/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.followersVisibility).toBe("public");
  expect(shown.followingVisibility).toBe("public");
  expect(typeof shown.followersCount).toBe("number");
  expect(typeof shown.followingCount).toBe("number");
});

test("Misskey互換API: i/notificationsのリアクション通知でローカルユーザーのavatarUrlが解決される（#74）", async ({
  request,
}) => {
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  let adminToken: string | null = null;
  try {
    const alice = await registerUserViaApi(request, "e2emknotifa");
    const bob = await registerUserViaApi(request, "e2emknotifb");
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-stub-${Date.now()}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), `ストレージプロバイダー登録失敗: ${providerRes.status()} ${await providerRes.text()}`).toBeTruthy();
    providerId = (await providerRes.json()).id;

    const uploadRes = await request.post("/api/drive/files/create", {
      headers: { Authorization: `Bearer ${bob.token}` },
      multipart: { file: { name: "avatar.png", mimeType: "image/png", buffer: MINIMAL_PNG }, media_type: "avatar" },
    });
    expect(uploadRes.ok(), `アバターアップロード失敗: ${uploadRes.status()} ${await uploadRes.text()}`).toBeTruthy();
    const uploaded = await uploadRes.json();

    const profileRes = await request.patch("/api/users/profile", {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { avatar_media_id: uploaded.id },
    });
    expect(profileRes.ok(), `プロフィール更新失敗: ${profileRes.status()} ${await profileRes.text()}`).toBeTruthy();

    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { text: `通知アイコンテスト ${Date.now()}` },
    });
    expect(createRes.ok()).toBeTruthy();
    const created = await createRes.json();

    const reactRes = await request.post(`/api/notes/${created.id}/reactions`, {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { content: "🎉" },
    });
    expect(reactRes.ok(), `リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

    const notifRes = await request.post("/api/i/notifications", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: {},
    });
    expect(notifRes.ok(), `i/notifications失敗: ${notifRes.status()} ${await notifRes.text()}`).toBeTruthy();
    const notifications = await notifRes.json();

    const reactionNotif = notifications.find((n: { type: string }) => n.type === "reaction");
    expect(reactionNotif, "リアクション通知が見つからない").toBeTruthy();
    expect(reactionNotif.user.avatarUrl, "ローカルユーザーのavatarUrlが解決されていない").not.toBeNull();
    expect(reactionNotif.user.avatarUrl).toContain(s3.url);
  } finally {
    if (providerId && adminToken) {
      await request.patch(`/api/admin/storage-providers/${providerId}`, {
        headers: { Authorization: `Bearer ${adminToken}` },
        data: { is_active: false },
      });
    }
    await s3.close();
  }
});

test("Misskey互換API: notes/mentionsでAriaの「メンション」「指名」タブに該当する投稿を返す", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emkmenta");
  const bob = await registerUserViaApi(request, "e2emkmentb");

  // bobへの通常メンション（本文中の@username）。
  const mentionRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `@${bob.username} メンションテスト ${Date.now()}`, visibility: "public" },
  });
  expect(mentionRes.ok(), `メンション投稿失敗: ${mentionRes.status()} ${await mentionRes.text()}`).toBeTruthy();
  const mentionNote = await mentionRes.json();

  // bobの投稿へのリプライ（本文中に@mention表記は無い）。
  const bobPostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { text: `bobの投稿 ${Date.now()}`, visibility: "public" },
  });
  expect(bobPostRes.ok()).toBeTruthy();
  const bobPost = await bobPostRes.json();

  const replyRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `返信テスト ${Date.now()}`, visibility: "public", reply_to_id: bobPost.id },
  });
  expect(replyRes.ok(), `リプライ投稿失敗: ${replyRes.status()} ${await replyRes.text()}`).toBeTruthy();
  const replyNote = await replyRes.json();

  // bob宛のDM（recipient_actor_idsで宛先指定、本文に@mention表記は無い。新規スレッドの
  // 最初の1通のためreply通知も発生しない、post_recipients側でのみ拾えるケース）。
  const dmRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `DMテスト ${Date.now()}`, visibility: "direct", recipient_actor_ids: [bob.actorId] },
  });
  expect(dmRes.ok(), `DM作成失敗: ${dmRes.status()} ${await dmRes.text()}`).toBeTruthy();
  const dmNote = await dmRes.json();

  // bobと無関係な投稿（どちらのタブにも出てはならない）。
  const unrelatedRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `無関係な投稿 ${Date.now()}`, visibility: "public" },
  });
  expect(unrelatedRes.ok()).toBeTruthy();
  const unrelatedNote = await unrelatedRes.json();

  // 「メンション」タブ（visibility省略）: メンション・リプライ・DMの3件。
  const mentionsTabRes = await request.post("/api/notes/mentions", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: {},
  });
  expect(mentionsTabRes.ok(), `notes/mentions失敗: ${mentionsTabRes.status()} ${await mentionsTabRes.text()}`).toBeTruthy();
  const mentionsTabIds = (await mentionsTabRes.json()).map((n: { id: string }) => n.id);
  expect(mentionsTabIds).toEqual(expect.arrayContaining([mentionNote.id, replyNote.id, dmNote.id]));
  expect(mentionsTabIds).not.toContain(unrelatedNote.id);

  // 「指名」タブ（visibility: "specified"）: DMのみ。
  const specifiedTabRes = await request.post("/api/notes/mentions", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { visibility: "specified" },
  });
  expect(specifiedTabRes.ok()).toBeTruthy();
  const specifiedTabIds = (await specifiedTabRes.json()).map((n: { id: string }) => n.id);
  expect(specifiedTabIds).toEqual([dmNote.id]);
});

test("Misskey互換API: 未実装機能（お知らせ・ハイライト・クリップ・ページ・Play・ギャラリー）は空配列を返す（#251）", async ({
  request,
}) => {
  const emptyArrayEndpoints = [
    "/api/announcements",
    "/api/users/featured-notes",
    "/api/users/clips",
    "/api/users/pages",
    "/api/users/flashs",
    "/api/users/gallery/posts",
  ];
  for (const path of emptyArrayEndpoints) {
    const res = await request.post(path, { data: { userId: "1" } });
    expect(res.ok(), `${path}失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
    expect(await res.json(), `${path}は空配列を返すべき`).toEqual([]);
  }
});

test("Misskey互換API: users/lists/listで自分の全リスト・他人の公開リストのみを返す（#251続き）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emklistsa");
  const bob = await registerUserViaApi(request, "e2emklistsb");

  const publicName = `公開リスト ${Date.now()}`;
  const privateName = `非公開リスト ${Date.now()}`;
  const publicListRes = await request.post("/api/lists", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { name: publicName, is_public: true },
  });
  expect(publicListRes.ok(), `公開リスト作成失敗: ${publicListRes.status()} ${await publicListRes.text()}`).toBeTruthy();
  const publicList = await publicListRes.json();

  const privateListRes = await request.post("/api/lists", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { name: privateName, is_public: false },
  });
  expect(privateListRes.ok(), `非公開リスト作成失敗: ${privateListRes.status()} ${await privateListRes.text()}`).toBeTruthy();
  const privateList = await privateListRes.json();

  const addMemberRes = await request.post(`/api/lists/${publicList.id}/members`, {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(addMemberRes.ok(), `メンバー追加失敗: ${addMemberRes.status()} ${await addMemberRes.text()}`).toBeTruthy();

  // 自分自身（userId省略）: 公開・非公開の両方が返る。
  const ownRes = await request.post("/api/users/lists/list", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: {},
  });
  expect(ownRes.ok(), `自分のリスト取得失敗: ${ownRes.status()} ${await ownRes.text()}`).toBeTruthy();
  const ownLists = await ownRes.json();
  const ownIds = ownLists.map((l: { id: string }) => l.id);
  expect(ownIds).toEqual(expect.arrayContaining([String(publicList.id), String(privateList.id)]));
  const ownPublic = ownLists.find((l: { id: string }) => l.id === String(publicList.id));
  expect(ownPublic.name).toBe(publicName);
  expect(ownPublic.isPublic).toBe(true);
  expect(ownPublic.userIds).toEqual([bob.actorId]);

  // 他人（userId指定）: 公開リストのみ、非公開は含まれない。
  const othersRes = await request.post("/api/users/lists/list", {
    data: { userId: alice.actorId },
  });
  expect(othersRes.ok(), `他人のリスト取得失敗: ${othersRes.status()} ${await othersRes.text()}`).toBeTruthy();
  const othersLists = await othersRes.json();
  const othersIds = othersLists.map((l: { id: string }) => l.id);
  expect(othersIds).toContain(String(publicList.id));
  expect(othersIds).not.toContain(String(privateList.id));
});

test("Misskey互換API: notes/user-list-timelineでリストを開いた画面が404にならず投稿一覧を返す（#251続き）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkultla");
  const bob = await registerUserViaApi(request, "e2emkultlb");
  const carol = await registerUserViaApi(request, "e2emkultlc");

  const listRes = await request.post("/api/lists", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { name: `TLテスト ${Date.now()}`, is_public: true },
  });
  expect(listRes.ok(), `リスト作成失敗: ${listRes.status()} ${await listRes.text()}`).toBeTruthy();
  const list = await listRes.json();

  const addMemberRes = await request.post(`/api/lists/${list.id}/members`, {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(addMemberRes.ok(), `メンバー追加失敗: ${addMemberRes.status()} ${await addMemberRes.text()}`).toBeTruthy();

  const memberText = `リストメンバーの投稿 ${Date.now()}`;
  const memberPostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { text: memberText },
  });
  expect(memberPostRes.ok()).toBeTruthy();
  const memberPost = await memberPostRes.json();

  // リストに入っていないユーザーの投稿は含まれてはならない。
  const unrelatedRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${carol.token}` },
    data: { text: `無関係な投稿 ${Date.now()}` },
  });
  expect(unrelatedRes.ok()).toBeTruthy();
  const unrelatedPost = await unrelatedRes.json();

  const tlRes = await request.post("/api/notes/user-list-timeline", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { listId: list.id },
  });
  expect(tlRes.ok(), `リストTL取得失敗: ${tlRes.status()} ${await tlRes.text()}`).toBeTruthy();
  const tlIds = (await tlRes.json()).map((n: { id: string }) => n.id);
  expect(tlIds).toContain(String(memberPost.id));
  expect(tlIds).not.toContain(String(unrelatedPost.id));

  // 存在しないリストIDは404（NO_SUCH_LIST）。
  const notFoundRes = await request.post("/api/notes/user-list-timeline", {
    data: { listId: "999999999999999999" },
  });
  expect(notFoundRes.status()).toBe(404);
});

test("Misskey互換API: users/lists/showでリスト詳細が404にならず取得できる（#251続き）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkulsa");
  const bob = await registerUserViaApi(request, "e2emkulsb");

  const publicListRes = await request.post("/api/lists", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { name: `詳細テスト公開 ${Date.now()}`, is_public: true },
  });
  expect(publicListRes.ok()).toBeTruthy();
  const publicList = await publicListRes.json();

  const privateListRes = await request.post("/api/lists", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { name: `詳細テスト非公開 ${Date.now()}`, is_public: false },
  });
  expect(privateListRes.ok()).toBeTruthy();
  const privateList = await privateListRes.json();

  const addMemberRes = await request.post(`/api/lists/${publicList.id}/members`, {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(addMemberRes.ok()).toBeTruthy();

  // 所有者本人: 公開リストの詳細を取得できる（メンバーID込み）。
  const showRes = await request.post("/api/users/lists/show", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { listId: publicList.id },
  });
  expect(showRes.ok(), `リスト詳細取得失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();
  expect(shown.id).toBe(String(publicList.id));
  expect(shown.isPublic).toBe(true);
  expect(shown.userIds).toEqual([bob.actorId]);

  // 第三者（未認証）: 公開リストは見えるが、非公開リストはNO_SUCH_LISTで404。
  const anonPublicRes = await request.post("/api/users/lists/show", {
    data: { listId: publicList.id },
  });
  expect(anonPublicRes.ok()).toBeTruthy();

  const anonPrivateRes = await request.post("/api/users/lists/show", {
    data: { listId: privateList.id },
  });
  expect(anonPrivateRes.status()).toBe(404);

  // 存在しないリストIDも404。
  const notFoundRes = await request.post("/api/users/lists/show", {
    data: { listId: "999999999999999999" },
  });
  expect(notFoundRes.status()).toBe(404);
});

test("Misskey互換API: users/showでuserIds指定時は配列を、単体指定時は従来通りオブジェクトを返す（#251続き）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkshowa");
  const bob = await registerUserViaApi(request, "e2emkshowb");

  // userIds一括指定（MisskeyUsers.showByIds、リストメンバー一覧画面）: 配列で返る。
  const batchRes = await request.post("/api/users/show", {
    data: { userIds: [alice.actorId, bob.actorId] },
  });
  expect(batchRes.ok(), `一括取得失敗: ${batchRes.status()} ${await batchRes.text()}`).toBeTruthy();
  const batch = await batchRes.json();
  expect(Array.isArray(batch)).toBe(true);
  const batchIds = batch.map((u: { id: string }) => u.id);
  expect(batchIds).toEqual(expect.arrayContaining([alice.actorId, bob.actorId]));

  // 従来通りuserId単体指定: オブジェクト（配列でない）で返る。
  const singleRes = await request.post("/api/users/show", {
    data: { userId: alice.actorId },
  });
  expect(singleRes.ok(), await singleRes.text()).toBeTruthy();
  const single = await singleRes.json();
  expect(Array.isArray(single)).toBe(false);
  expect(single.id).toBe(alice.actorId);
  expect(single.username).toBe(alice.username);
});

test("Misskey互換API: ap/showで「ほかのアカウントで開く」機能がノートを解決できる（#251続き）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkapshowa");

  const noteText = `ap/showテスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: noteText },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const note = await createRes.json();

  // ローカル投稿は自己参照的なAP Object IDを常に持つため、ネットワークフェッチ無しで
  // DB照合だけで解決できる（`open_activitypub_url`の`find_id_by_ap_or_at_uri`短絡）。
  const showRes = await request.post("/api/ap/show", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { uri: `https://localhost/notes/${note.id}` },
  });
  expect(showRes.ok(), `ap/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();
  expect(shown.type).toBe("Note");
  expect(shown.object.id).toBe(String(note.id));
  expect(shown.object.text).toBe(noteText);

  // 解決不能なuri（URLとしてすら不正）は400。
  const invalidRes = await request.post("/api/ap/show", {
    data: { uri: "not a valid uri at all" },
  });
  expect(invalidRes.status()).toBe(400);
});

test("Misskey互換API: リポストのnotes/showで埋め込まれた元ノートの本文カスタム絵文字が展開される（#252）", async ({
  request,
}) => {
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  let adminToken: string | null = null;
  try {
    const alice = await registerUserViaApi(request, "e2emkremoji1");
    const bob = await registerUserViaApi(request, "e2emkremoji2");
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-misskey-repost-emoji-${Date.now()}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), await providerRes.text()).toBeTruthy();
    providerId = (await providerRes.json()).id;

    const uploadRes = await request.post("/api/drive/files/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      multipart: {
        file: { name: "emoji.png", mimeType: "image/png", buffer: MINIMAL_PNG },
        media_type: "emoji",
      },
    });
    expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
    const uploaded = await uploadRes.json();

    const shortcode = `mkremoji${Date.now().toString(36)}`;
    const emojiRes = await request.post("/api/admin/emojis", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: { shortcode, media_file_id: uploaded.id },
    });
    expect(emojiRes.ok(), await emojiRes.text()).toBeTruthy();

    // 本文にカスタム絵文字ショートコードを含む投稿。
    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { text: `本文絵文字テスト :${shortcode}:` },
    });
    expect(createRes.ok(), await createRes.text()).toBeTruthy();
    const original = await createRes.json();

    // それをプレーンリポストする。
    const repostRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { renote_id: original.id },
    });
    expect(repostRes.ok(), `リポスト作成失敗: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();
    const repost = await repostRes.json();

    const showRes = await request.post("/api/notes/show", {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { noteId: repost.id },
    });
    expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
    const shown = await showRes.json();

    expect(shown.renote, "renote本体がnull").not.toBeNull();
    expect(
      shown.renote.emojis[shortcode],
      "埋め込まれた元ノートのemojisにショートコードが無い（本文カスタム絵文字が展開されない原因、#252）",
    ).toBeTruthy();
  } finally {
    if (providerId && adminToken) {
      await request.patch(`/api/admin/storage-providers/${providerId}`, {
        headers: { Authorization: `Bearer ${adminToken}` },
        data: { is_active: false },
      });
    }
    await s3.close();
  }
});

test("Misskey互換API: statsのnotesCount/usersCountが退会済みユーザー・削除済みポスト・リモートポストを除外したローカル実数を返す（#251）", async ({
  request,
}) => {
  const getStats = async () => {
    const res = await request.post("/api/stats", { data: {} });
    expect(res.ok(), await res.text()).toBeTruthy();
    return (await res.json()) as {
      notesCount: number;
      originalNotesCount: number;
      usersCount: number;
      originalUsersCount: number;
      instances: number;
      driveUsageLocal: number;
      driveUsageRemote: number;
    };
  };

  const before = await getStats();
  // notesCount/usersCountはリモートを一切集計しないため常にoriginalと同値。
  expect(before.notesCount).toBe(before.originalNotesCount);
  expect(before.usersCount).toBe(before.originalUsersCount);
  expect(before.instances).toBe(0);
  expect(before.driveUsageLocal).toBe(0);
  expect(before.driveUsageRemote).toBe(0);

  const alice = await registerUserViaApi(request, "e2emkstatsa");
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `stats集計テスト ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const note = await createRes.json();

  // 新規ローカルユーザー1人・新規投稿1件で、それぞれ+1されることを確認する。
  const afterCreate = await getStats();
  expect(afterCreate.usersCount).toBe(before.usersCount + 1);
  expect(afterCreate.originalUsersCount).toBe(before.usersCount + 1);
  expect(afterCreate.notesCount).toBe(before.notesCount + 1);
  expect(afterCreate.originalNotesCount).toBe(before.notesCount + 1);

  // 投稿を削除すると、削除済みポストとして集計から外れる（ユーザー数は変わらない）。
  const deleteRes = await request.delete(`/api/notes/${note.id}`, {
    headers: { Authorization: `Bearer ${alice.token}` },
  });
  expect(deleteRes.ok(), `投稿削除失敗: ${deleteRes.status()} ${await deleteRes.text()}`).toBeTruthy();

  const afterDelete = await getStats();
  expect(afterDelete.notesCount).toBe(before.notesCount);
  expect(afterDelete.usersCount).toBe(before.usersCount + 1);
});

test("Misskey互換API: users/reactionsでプロフィール「リアクション」タブに対象ノート付きの一覧を返す（#251）", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emkreacusera");
  const bob = await registerUserViaApi(request, "e2emkreacuserb");

  const noteText = `リアクション対象ポスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: noteText },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const note = await createRes.json();

  const reactRes = await request.post("/api/notes/reactions/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { noteId: note.id, reaction: "👍" },
  });
  expect(reactRes.ok(), `リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

  const listRes = await request.post("/api/users/reactions", {
    data: { userId: bob.actorId, limit: 10 },
  });
  expect(listRes.ok(), `users/reactions失敗: ${listRes.status()} ${await listRes.text()}`).toBeTruthy();
  const list = await listRes.json();

  expect(list).toHaveLength(1);
  expect(list[0].type).toBe("👍");
  expect(list[0].user.username).toBe(bob.username);
  expect(list[0].note.id).toBe(String(note.id));
  expect(list[0].note.text).toBe(noteText);
});
