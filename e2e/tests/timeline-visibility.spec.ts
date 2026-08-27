import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";

test("ひかえめ・プライベート投稿はLTL/GTLに本人にも出ず、HTL/STLには表示される", async ({ request }) => {
  const author = await registerUserViaApi(request, "e2eprivateauthor");
  const follower = await registerUserViaApi(request, "e2eprivatefollower");

  const follow = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${follower.token}` },
    data: { target: author.username },
  });
  expect(follow.ok(), `follow failed: ${follow.status()} ${await follow.text()}`).toBeTruthy();

  const stamp = Date.now();
  const posts = [
    { text: `unlisted timeline test ${stamp}`, visibility: "home" },
    { text: `private timeline test ${stamp}`, visibility: "followers" },
  ];
  for (const post of posts) {
    const created = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${author.token}` },
      data: post,
    });
    expect(created.ok(), `create failed: ${created.status()} ${await created.text()}`).toBeTruthy();
  }

  async function timeline(path: string, token: string) {
    const response = await request.get(path, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok(), `${path} failed: ${response.status()} ${await response.text()}`).toBeTruthy();
    return (await response.json()) as { text: string }[];
  }

  for (const viewer of [author, follower]) {
    const local = await timeline("/api/notes/local-timeline?limit=100", viewer.token);
    const global = await timeline("/api/notes/global-timeline?limit=100", viewer.token);
    const home = await timeline("/api/notes/home-timeline?limit=100", viewer.token);
    const social = await timeline("/api/notes/social-timeline?limit=100", viewer.token);
    for (const post of posts) {
      expect(local.some((note) => note.text === post.text)).toBe(false);
      expect(global.some((note) => note.text === post.text)).toBe(false);
      expect(home.some((note) => note.text === post.text)).toBe(true);
      expect(social.some((note) => note.text === post.text)).toBe(true);
    }
  }
});

// フォロー中ユーザーの投稿でも、リプライについてはリプライ先投稿者も
// フォロー中（または自分自身）でなければホームタイムラインには表示しない
// （post_reply_target_followed）。ソーシャルタイムラインはローカル全体パートで
// 無条件にリプライも拾うため対象外（フォロー中パートのみの絞り込み）。
test("HTLはフォロー中ユーザーのリプライでもリプライ先未フォローなら表示しない、STLは表示する", async ({
  request,
}) => {
  const viewer = await registerUserViaApi(request, "e2ahtlreplyviewer");
  const followed = await registerUserViaApi(request, "e2ahtlreplyfollowed");
  const unfollowed = await registerUserViaApi(request, "e2ahtlreplyunfollowed");

  const follow = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${viewer.token}` },
    data: { target: followed.username },
  });
  expect(follow.ok(), `follow failed: ${follow.status()} ${await follow.text()}`).toBeTruthy();

  const stamp = Date.now();
  async function createNote(token: string, text: string, replyToId?: string) {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${token}` },
      data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public", reply_to_id: replyToId },
    });
    expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
    return res.json();
  }

  const unfollowedPost = await createNote(unfollowed.token, `未フォロー元投稿 ${stamp}`);
  const replyToUnfollowedText = `フォロー中→未フォローへの返信 ${stamp}`;
  await createNote(followed.token, replyToUnfollowedText, unfollowedPost.id);
  const normalText = `フォロー中の通常投稿 ${stamp}`;
  await createNote(followed.token, normalText);

  async function timeline(path: string) {
    const response = await request.get(path, {
      headers: { Authorization: `Bearer ${viewer.token}` },
    });
    expect(response.ok(), `${path} failed: ${response.status()} ${await response.text()}`).toBeTruthy();
    return (await response.json()) as { text: string }[];
  }

  const home = await timeline("/api/notes/home-timeline?limit=100");
  const social = await timeline("/api/notes/social-timeline?limit=100");

  expect(home.some((note) => note.text === replyToUnfollowedText)).toBe(false);
  expect(home.some((note) => note.text === normalText)).toBe(true);
  expect(social.some((note) => note.text === replyToUnfollowedText)).toBe(true);
  expect(social.some((note) => note.text === normalText)).toBe(true);
});
