import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";

test("プロフィール投稿一覧では公開投稿のリポスト元が埋め込まれる（#115）", async ({ request }) => {
  const author = await registerUserViaApi(request, "e2eprofrenoteauthor");
  const reposter = await registerUserViaApi(request, "e2eprofrenotereposter");
  const text = `プロフィールのリポスト元 ${Date.now()}`;

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text },
  });
  expect(createRes.ok(), await createRes.text()).toBeTruthy();
  const original = (await createRes.json()) as { id: string };

  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${reposter.token}` },
    data: { renote_id: original.id },
  });
  expect(repostRes.ok(), await repostRes.text()).toBeTruthy();
  const repost = (await repostRes.json()) as { id: string };

  const profileRes = await request.get(`/api/users/profile?q=${encodeURIComponent(reposter.username)}`, {
    headers: { Authorization: `Bearer ${author.token}` },
  });
  expect(profileRes.ok(), await profileRes.text()).toBeTruthy();
  const profile = (await profileRes.json()) as {
    recent_posts: Array<{ id: string; renote_id?: string; renote?: { id: string; text: string } }>;
  };
  const initialRepost = profile.recent_posts.find((note) => note.id === String(repost.id));
  expect(initialRepost?.renote_id).toBe(String(original.id));
  expect(initialRepost?.renote).toMatchObject({ id: String(original.id), text });

  const postsRes = await request.get(
    `/api/users/posts?actor_id=${encodeURIComponent(reposter.actorId)}&limit=20&exclude_direct=true`,
    { headers: { Authorization: `Bearer ${author.token}` } },
  );
  expect(postsRes.ok(), await postsRes.text()).toBeTruthy();
  const posts = (await postsRes.json()) as Array<{
    id: string;
    renote_id?: string;
    renote?: { id: string; text: string };
  }>;
  const pagedRepost = posts.find((note) => note.id === String(repost.id));
  expect(pagedRepost?.renote_id).toBe(String(original.id));
  expect(pagedRepost?.renote).toMatchObject({ id: String(original.id), text });
});
