import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";

test("フォロワー限定投稿はLTL/GTLに出ず、STLには表示される", async ({ request }) => {
  const author = await registerUserViaApi(request, "e2eprivateauthor");
  const follower = await registerUserViaApi(request, "e2eprivatefollower");

  const follow = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${follower.token}` },
    data: { target: author.username },
  });
  expect(follow.ok(), `follow failed: ${follow.status()} ${await follow.text()}`).toBeTruthy();

  const text = `private timeline test ${Date.now()}`;
  const created = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text, visibility: "followers" },
  });
  expect(created.ok(), `create failed: ${created.status()} ${await created.text()}`).toBeTruthy();

  async function timeline(path: string) {
    const response = await request.get(path, {
      headers: { Authorization: `Bearer ${follower.token}` },
    });
    expect(response.ok(), `${path} failed: ${response.status()} ${await response.text()}`).toBeTruthy();
    return (await response.json()) as { text: string }[];
  }

  expect((await timeline("/api/notes/local-timeline?limit=100")).some((note) => note.text === text)).toBe(false);
  expect((await timeline("/api/notes/global-timeline?limit=100")).some((note) => note.text === text)).toBe(false);
  expect((await timeline("/api/notes/social-timeline?limit=100")).some((note) => note.text === text)).toBe(true);
});
