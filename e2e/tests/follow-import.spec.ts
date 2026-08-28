import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";

interface FollowImportStatus {
  status: "idle" | "running" | "completed" | "cancelled";
  total: number;
  processed: number;
  succeeded: number;
  failed: number;
}

async function pollUntilDone(
  request: import("@playwright/test").APIRequestContext,
  token: string,
): Promise<FollowImportStatus> {
  let status: FollowImportStatus | undefined;
  await expect
    .poll(
      async () => {
        const res = await request.get("/api/account/follow-import", {
          headers: { Authorization: `Bearer ${token}` },
        });
        expect(res.ok(), await res.text()).toBeTruthy();
        status = (await res.json()) as FollowImportStatus;
        return status.status;
      },
      { timeout: 30_000, intervals: [300] },
    )
    .not.toBe("running");
  return status!;
}

test("改行区切りのユーザー名を一括フォローでき、進捗が完了まで反映される", async ({ request }) => {
  const importer = await registerUserViaApi(request, "e2fimp1");
  const targetA = await registerUserViaApi(request, "e2fimptga");
  const targetB = await registerUserViaApi(request, "e2fimptgb");

  const startRes = await request.post("/api/account/follow-import", {
    headers: { Authorization: `Bearer ${importer.token}` },
    data: { text: `${targetA.username}\n${targetB.username}` },
  });
  expect(startRes.ok(), await startRes.text()).toBeTruthy();
  const started = (await startRes.json()) as { requestId: number; total: number };
  expect(started.total).toBe(2);

  const finalStatus = await pollUntilDone(request, importer.token);
  expect(finalStatus.status).toBe("completed");
  expect(finalStatus.succeeded).toBe(2);
  expect(finalStatus.failed).toBe(0);

  // 実際にフォロー関係が成立していることを確認（ローカル同士は即accepted）。
  const followingRes = await request.get(
    `/api/users/following?actor_id=${importer.actorId}&limit=10`,
    { headers: { Authorization: `Bearer ${importer.token}` } },
  );
  expect(followingRes.ok(), await followingRes.text()).toBeTruthy();
  const following = (await followingRes.json()) as Array<{ username: string }>;
  const usernames = following.map((a) => a.username);
  expect(usernames).toContain(targetA.username);
  expect(usernames).toContain(targetB.username);
});

test("隠し仕様: 各行をカンマ区切りで分割し1列目のみを識別子として読む（Misskeyエクスポート対応）", async ({
  request,
}) => {
  const importer = await registerUserViaApi(request, "e2fimp2");
  const target = await registerUserViaApi(request, "e2fimptgc");

  // Misskeyのフォローエクスポート形式（ヘッダ無し `id,withRepliesフラグ`）を模した行。
  const startRes = await request.post("/api/account/follow-import", {
    headers: { Authorization: `Bearer ${importer.token}` },
    data: { text: `${target.username},false` },
  });
  expect(startRes.ok(), await startRes.text()).toBeTruthy();

  const finalStatus = await pollUntilDone(request, importer.token);
  expect(finalStatus.status).toBe("completed");
  expect(finalStatus.succeeded).toBe(1);
});

// 解決に失敗する架空のIDを大量に混ぜ、ジョブが1件処理するたびのDBラウンドトリップ回数を
// 稼ぐことで、テスト側の後続リクエストが届く前に「running」状態を通り過ぎてしまう
// レースコンディションを避ける（ローカル実在ユーザーへのフォローは数件で一瞬で完了してしまう）。
function manyUnresolvableTargets(count: number, prefix: string): string {
  return Array.from({ length: count }, (_, i) => `${prefix}-nonexistent-${i}`).join("\n");
}

test("実行中に再度開始しようとするとConflictになる", async ({ request }) => {
  const importer = await registerUserViaApi(request, "e2fimp3");

  const firstRes = await request.post("/api/account/follow-import", {
    headers: { Authorization: `Bearer ${importer.token}` },
    data: { text: manyUnresolvableTargets(300, "conflict") },
  });
  expect(firstRes.ok(), await firstRes.text()).toBeTruthy();

  const secondRes = await request.post("/api/account/follow-import", {
    headers: { Authorization: `Bearer ${importer.token}` },
    data: { text: "somebody" },
  });
  expect(secondRes.status()).toBe(409);

  await pollUntilDone(request, importer.token);
});

test("キャンセルすると残りが処理されず未処理のまま止まる", async ({ request }) => {
  const importer = await registerUserViaApi(request, "e2fimp4");

  const startRes = await request.post("/api/account/follow-import", {
    headers: { Authorization: `Bearer ${importer.token}` },
    data: { text: manyUnresolvableTargets(300, "cancel") },
  });
  expect(startRes.ok(), await startRes.text()).toBeTruthy();

  const cancelRes = await request.post("/api/account/follow-import/cancel", {
    headers: { Authorization: `Bearer ${importer.token}` },
  });
  expect(cancelRes.ok(), await cancelRes.text()).toBeTruthy();

  await expect
    .poll(
      async () => {
        const res = await request.get("/api/account/follow-import", {
          headers: { Authorization: `Bearer ${importer.token}` },
        });
        const s = (await res.json()) as FollowImportStatus;
        return s.status;
      },
      { timeout: 10_000, intervals: [300] },
    )
    .toBe("cancelled");

  // キャンセル後、新たにインポートを再開できる（実行中扱いが残っていない）ことも確認。
  const restartRes = await request.post("/api/account/follow-import", {
    headers: { Authorization: `Bearer ${importer.token}` },
    data: { text: "somebody-else" },
  });
  expect(restartRes.ok(), await restartRes.text()).toBeTruthy();
  await pollUntilDone(request, importer.token);
});
