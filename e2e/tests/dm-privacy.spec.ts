// DMの可視性漏洩防止を横断的に固定するテスト。2026-09-15、post_is_visible_to の
// パラメータ/列名衝突バグ（direct可視性チェックが「このDMの宛先か」ではなく
// 「過去に何かDMを受け取ったことがあるか」という無関係な判定に壊れていた）が原因で、
// 無関係な第三者が他人同士のDMを閲覧・リアクションできてしまう実害が発生した
// （docs/protocols.md 9節、docs/database.md 参照）。同種の回帰を検知できるよう、
// 「DM受信経験のある第三者」と「DM経験の無い第三者」の両方が、タイムライン・URL直指定・
// プロフィールページ・検索・リアクション・リプライ・DM API のいずれからも他人のDMへ
// 到達できないことを固定する。

import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test.describe("無関係な第三者は他人のDMへどの経路からも到達できない", () => {
  test("タイムライン・URL直指定・プロフィール・検索・リアクション・リプライ・DM APIすべてで漏洩しない", async ({
    page,
    request,
  }) => {
    const alice = await registerUserViaApi(request, "e2dmprivA");
    const bob = await registerUserViaApi(request, "e2dmprivB");
    // charlie: 「DM受信経験のある第三者」（過去バグの再現条件そのもの）。
    // dave: charlieにDM履歴を持たせるためだけの相手。
    const charlie = await registerUserViaApi(request, "e2dmprivC");
    const dave = await registerUserViaApi(request, "e2dmprivD");
    // erin: DM経験が一切無い、素の無関係ユーザー（より基本的な回帰の安全網）。
    const erin = await registerUserViaApi(request, "e2dmprivE");

    // charlieとdaveの間に無関係なDMを1件作っておく（charlieをpost_recipientsに登録させる）。
    const unrelatedDmRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${dave.token}` },
      data: { text: `無関係DM ${Date.now()}`, visibility: "direct", recipient_actor_ids: [charlie.actorId] },
    });
    expect(unrelatedDmRes.ok()).toBeTruthy();

    // alice→bobの本命DM。本文はユニークな文字列にして検索テストにも使う。
    const dmText = `プライベートDM本文 ${Date.now()} ${Math.random().toString(36).slice(2)}`;
    const dmRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { text: dmText, visibility: "direct", recipient_actor_ids: [bob.actorId] },
    });
    expect(dmRes.ok(), `DM作成失敗: ${dmRes.status()} ${await dmRes.text()}`).toBeTruthy();
    const dm = await dmRes.json();
    const dmId: string = dm.id;

    for (const [label, outsider] of [
      ["DM受信経験あり(charlie)", charlie],
      ["DM経験無し(erin)", erin],
    ] as const) {
      const authHeaders = { Authorization: `Bearer ${outsider.token}` };

      // (a) GET /api/notes/:id — 直接取得は404。
      await test.step(`${label}: GET /api/notes/:id は404`, async () => {
        const res = await request.get(`/api/notes/${dmId}`, { headers: authHeaders });
        expect(res.status(), await res.text()).toBe(404);
      });

      // (b) 4種のタイムライン（Misskey互換、exclude_direct=false固定の経路）にDM本文が出ない。
      for (const path of ["/api/notes/timeline", "/api/notes/local-timeline", "/api/notes/hybrid-timeline", "/api/notes/global-timeline"]) {
        await test.step(`${label}: POST ${path} にDM本文が出ない`, async () => {
          const res = await request.post(path, { headers: authHeaders, data: { limit: 50 } });
          expect(res.ok(), await res.text()).toBeTruthy();
          const notes = (await res.json()) as { text?: string }[];
          expect(notes.some((n) => n.text === dmText)).toBeFalsy();
        });
      }

      // (c) プロフィールページの投稿一覧にDMが出ない。
      await test.step(`${label}: プロフィール投稿一覧にDMが出ない`, async () => {
        const res = await request.get(`/api/users/posts?actor_id=${encodeURIComponent(alice.actorId)}`, {
          headers: authHeaders,
        });
        expect(res.ok(), await res.text()).toBeTruthy();
        const posts = (await res.json()) as { text?: string }[];
        expect(posts.some((n) => n.text === dmText)).toBeFalsy();
      });

      // (d) 検索（ネイティブGET・Misskey互換POSTの両方）でDM本文がヒットしない。
      await test.step(`${label}: GET /api/notes/search でヒットしない`, async () => {
        const res = await request.get(`/api/notes/search?q=${encodeURIComponent(dmText)}`, { headers: authHeaders });
        expect(res.ok(), await res.text()).toBeTruthy();
        const body = (await res.json()) as { notes: { text: string }[] };
        expect(body.notes.some((n) => n.text === dmText)).toBeFalsy();
      });
      await test.step(`${label}: POST /api/notes/search（Misskey互換）でヒットしない`, async () => {
        const res = await request.post("/api/notes/search", {
          headers: authHeaders,
          data: { query: dmText },
        });
        expect(res.ok(), await res.text()).toBeTruthy();
        const notes = (await res.json()) as { text: string }[];
        expect(notes.some((n) => n.text === dmText)).toBeFalsy();
      });

      // (e) リアクションを付けられない（存在しないポストと同じ404扱い）。
      await test.step(`${label}: リアクション作成は404`, async () => {
        const res = await request.post(`/api/notes/${dmId}/reactions`, {
          headers: authHeaders,
          data: { content: "👍" },
        });
        expect(res.status(), await res.text()).toBe(404);
      });

      // (f) リプライを作れない（見えないポストへのリプライはREPLY_TARGET_NOT_FOUND）。
      await test.step(`${label}: リプライ作成は404`, async () => {
        const res = await request.post("/api/notes/create", {
          headers: authHeaders,
          data: { text: `勝手にリプライ ${Date.now()}`, reply_to_id: dmId, visibility: "public" },
        });
        expect(res.status(), await res.text()).toBe(404);
      });

      // (g) DMスレッドAPI: メッセージ取得は403、セッション一覧にも現れない。
      await test.step(`${label}: DMスレッドメッセージ取得は403`, async () => {
        const res = await request.get(`/api/dm/sessions/${dmId}/messages`, { headers: authHeaders });
        expect(res.status(), await res.text()).toBe(403);
      });
      await test.step(`${label}: DMセッション一覧に現れない`, async () => {
        const res = await request.get("/api/dm/sessions", { headers: authHeaders });
        expect(res.ok(), await res.text()).toBeTruthy();
        const sessions = (await res.json()) as { threadRootPostId: string }[];
        expect(sessions.some((s) => s.threadRootPostId === dmId)).toBeFalsy();
      });
    }

    // (h) URL直指定（ブラウザ）: 当事者ではないcharlieが/notes/:idを直接踏んでも
    // 本文は一切画面に出ない（NoteDetailPageはAPIの404をそのままエラー表示する）。
    await seedAuth(page, charlie.token);
    await page.goto(`/notes/${dmId}`);
    await expect(page.getByText(dmText)).toHaveCount(0, { timeout: 10_000 });
    // メッセージスレッドへ誤ってリダイレクトされてもいない（可視ではないため）。
    await expect(page).not.toHaveURL(new RegExp(`/messages/${dmId}$`));
  });
});
