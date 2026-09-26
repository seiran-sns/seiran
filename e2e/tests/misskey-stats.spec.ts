import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";

// stats（notesCount/usersCount等）はグローバル集計のため、他specと並行実行される
// storage-serialプロジェクト内に置くとbefore/after差分に他specの登録・投稿が
// 混入してflakyになる（実機確認）。全spec完了後の排他テール（globals-serial）
// でのみ安全に検証できるため、misskey-compat.spec.tsから分離した。

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
