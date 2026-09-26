// アンケートへの同時投票で票数が失われないこと、同じユーザーの同時二重投票が1票だけ
// 記録されることを固定する。以前は投票処理が「posts.poll（JSON）を読む→アプリで+1→
// 丸ごと書き戻す」形で、同時に投票すると一方の加算が失われていた（2026-09-26 改善大会 R1）。

import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";

test("アンケートへの同時投票で票数が失われず、同一ユーザーの同時二重投票は1票だけ数える", async ({
  request,
}) => {
  const author = await registerUserViaApi(request, "e2epollcc");
  const voters = await Promise.all(
    Array.from({ length: 6 }, () => registerUserViaApi(request, "e2epollccv")),
  );

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: {
      text: `同時投票テスト ${Date.now()}`,
      poll: { choices: ["A", "B"], expiresInSeconds: 3600 },
    },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  const vote = (token: string, optionIndexes: number[]) =>
    request.post(`/api/notes/${created.id}/poll-vote`, {
      headers: { Authorization: `Bearer ${token}` },
      data: { optionIndexes },
    });

  // 6人がAへ、投稿者がBへ同時に投票する。
  const results = await Promise.all([
    ...voters.map((v) => vote(v.token, [0])),
    vote(author.token, [1]),
  ]);
  for (const res of results) {
    expect(res.ok(), `投票失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  }

  // 同じユーザーが同時に2回投票しても、成功は1回だけ（もう1回は409）。
  const doubleVoter = await registerUserViaApi(request, "e2epollccd");
  const doubleResults = await Promise.all([vote(doubleVoter.token, [1]), vote(doubleVoter.token, [1])]);
  const statuses = doubleResults.map((r) => r.status()).sort();
  expect(statuses).toEqual([200, 409]);

  const getRes = await request.get(`/api/notes/${created.id}`, {
    headers: { Authorization: `Bearer ${author.token}` },
  });
  expect(getRes.ok()).toBeTruthy();
  const note = await getRes.json();
  expect(note.poll.options.map((o: { votes: number }) => o.votes)).toEqual([6, 2]);
});
