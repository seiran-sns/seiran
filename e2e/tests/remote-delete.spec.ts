import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

async function findSessionByText(request: import("@playwright/test").APIRequestContext, token: string, text: string) {
  const res = await request.get("/api/dm/sessions", { headers: { Authorization: `Bearer ${token}` } });
  if (!res.ok()) return undefined;
  const sessions = (await res.json()) as { threadRootPostId: string; lastMessage: { text: string } }[];
  return sessions.find((s) => s.lastMessage.text === text);
}

test.describe("Fedi受信投稿のリモート削除反映", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  test("Fediから受信済みの投稿がDelete受信で削除される", async ({ request }) => {
    const alice = await registerUserViaApi(request, "e2remotedel");

    const text = `Fedi削除テスト ${Date.now()}`;
    const noteId = await fedi.sendCreateNote(SEIRAN_BASE_URL, alice.username, text);

    // Inbox受信は非同期処理（Job::InboundActivityProcess）のため反映を待つ。
    // direct投稿はDMセッションとして現れる。
    await expect.poll(() => findSessionByText(request, alice.token, text), { timeout: 15_000 }).toBeTruthy();

    await fedi.sendDeleteNote(SEIRAN_BASE_URL, noteId);

    // 削除（deleted_at設定）後は唯一のメッセージだったスレッドごとセッション一覧から消える。
    await expect
      .poll(async () => (await findSessionByText(request, alice.token, text)) === undefined, { timeout: 15_000 })
      .toBeTruthy();
  });

  test("Delete送信元が投稿者本人でない場合は削除されない（なりすまし対策）", async ({ request }) => {
    const alice = await registerUserViaApi(request, "e2remotedelforge");
    const impostor = await startStubFediServer();

    try {
      const text = `Fedi削除なりすましテスト ${Date.now()}`;
      const noteId = await fedi.sendCreateNote(SEIRAN_BASE_URL, alice.username, text);

      await expect.poll(() => findSessionByText(request, alice.token, text), { timeout: 15_000 }).toBeTruthy();

      // 投稿者(fedi)本人ではなく、別アクター(impostor)からのDeleteは無視されるべき。
      await impostor.sendDeleteNote(SEIRAN_BASE_URL, noteId);

      // 少し待っても削除されていないことを確認する（ネガティブケースのため固定待機は避けられないが、
      // 非同期ジョブの処理が確実に一巡する時間として3秒を確保する）。
      await new Promise((r) => setTimeout(r, 3_000));
      const session = await findSessionByText(request, alice.token, text);
      expect(session, "なりすましDeleteでセッションが消えてしまっている").toBeTruthy();
    } finally {
      await impostor.close();
    }
  });
});
