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
