import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// ポスト詳細画面の「前後のポスト」タブ（#226）: タブを開くと同時に自動読み込みし、
// 最大5件ずつ表示。読み込みボタンで続きの1件（対象から遠い方）を追加取得できる。
test("前後の投稿タブが最大5件表示され読み込みボタンで続きが表示される", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ectxtabauth");
  const ts = Date.now();

  async function post(text: string) {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${author.token}` },
      data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
    });
    expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
    return res.json();
  }

  // 対象ポストの前に6件、後に6件投稿する（対象に近い5件だけが初回表示され、
  // 最も遠い1件は「読み込みボタン」を押すまで表示されない想定）。
  for (let i = 1; i <= 6; i++) {
    await post(`古い投稿${i} ${ts}`);
  }
  const target = await post(`対象ポスト ${ts}`);
  for (let i = 1; i <= 6; i++) {
    await post(`新しい投稿${i} ${ts}`);
  }

  await seedAuth(page, author.token);
  await page.goto(`/notes/${target.id}`);
  await expect(page.getByText(`対象ポスト ${ts}`)).toBeVisible({ timeout: 15_000 });

  const rightPane = page.getByRole("complementary");
  await rightPane.getByRole("button", { name: "前後のポスト", exact: true }).click();

  // 対象ポスト自身（拡大文字表示NoteCard）がリスト中央に含まれる。ボタン操作不要で自動読み込みされる。
  await expect(rightPane.getByText(`対象ポスト ${ts}`)).toBeVisible({ timeout: 15_000 });

  // 対象に近い5件ずつ（古い投稿2〜6、新しい投稿1〜5）が表示される。
  for (let i = 2; i <= 6; i++) {
    await expect(rightPane.getByText(`古い投稿${i} ${ts}`)).toBeVisible({ timeout: 15_000 });
  }
  for (let i = 1; i <= 5; i++) {
    await expect(rightPane.getByText(`新しい投稿${i} ${ts}`)).toBeVisible({ timeout: 15_000 });
  }
  // 最も遠い1件ずつはまだ表示されない。
  await expect(rightPane.getByText(`古い投稿1 ${ts}`)).toHaveCount(0);
  await expect(rightPane.getByText(`新しい投稿6 ${ts}`)).toHaveCount(0);

  await rightPane.getByRole("button", { name: "もっと古い投稿を読み込む", exact: true }).click();
  await expect(rightPane.getByText(`古い投稿1 ${ts}`)).toBeVisible({ timeout: 15_000 });

  await rightPane.getByRole("button", { name: "もっと新しい投稿を読み込む", exact: true }).click();
  await expect(rightPane.getByText(`新しい投稿6 ${ts}`)).toBeVisible({ timeout: 15_000 });
});
