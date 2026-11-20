import { expect, test, type APIRequestContext } from "@playwright/test";
import { ADMIN_PASSWORD, ADMIN_USERNAME, loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { uniquePng } from "../fixtures/png";
import { startStubS3Server } from "../fixtures/stub-s3-server";
import { buildZip, readZip } from "../fixtures/zip";

interface ImportStatus {
  status: string;
  total: number | null;
  processed: number;
  imported: number;
  skipped: number;
  failed: number;
  problems: { shortcode: string; status: string; reason: string }[];
}

async function importStatus(request: APIRequestContext, token: string): Promise<ImportStatus> {
  const res = await request.get("/api/admin/emojis/import", {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(res.ok(), await res.text()).toBeTruthy();
  return (await res.json()) as ImportStatus;
}

test("Misskey形式ZIPの絵文字インポートはジョブで進み、エクスポートしたZIPに取り込んだ絵文字が入る", async ({
  page,
  request,
}) => {
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const auth = { Authorization: `Bearer ${adminToken}` };
  const suffix = Date.now().toString(36);
  const good = `e2ezip_${suffix}`;
  const missing = `e2ezip_missing_${suffix}`;
  let importedId: string | null = null;
  try {
    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: auth,
      data: {
        name: `e2e-emoji-zip-${suffix}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), await providerRes.text()).toBeTruthy();
    providerId = (await providerRes.json()).id;

    const meta = {
      metaVersion: 2,
      host: "misskey.example",
      emojis: [
        {
          fileName: `${good}.png`,
          downloaded: true,
          emoji: { name: good, category: "e2e", aliases: ["e2e_alias"], license: "CC0" },
        },
        { fileName: "bad.png", downloaded: true, emoji: { name: "bad-name", aliases: [] } },
        { fileName: `${missing}.png`, downloaded: true, emoji: { name: missing, aliases: [] } },
      ],
    };
    const zip = buildZip([
      { name: "meta.json", data: Buffer.from(JSON.stringify(meta)) },
      { name: `${good}.png`, data: uniquePng() },
      { name: "bad.png", data: uniquePng() },
    ]);

    const startRes = await request.post("/api/admin/emojis/import", {
      headers: auth,
      multipart: { file: { name: "emojis.zip", mimeType: "application/zip", buffer: zip } },
    });
    expect(startRes.ok(), await startRes.text()).toBeTruthy();

    await expect
      .poll(async () => (await importStatus(request, adminToken)).status, { timeout: 30_000 })
      .toBe("completed");
    const done = await importStatus(request, adminToken);
    expect(done).toMatchObject({ total: 3, processed: 3, imported: 1, skipped: 1, failed: 1 });
    expect(done.problems).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ shortcode: "bad-name", status: "skipped", reason: "invalid_shortcode" }),
        expect.objectContaining({ shortcode: missing, status: "failed", reason: "file_missing" }),
      ]),
    );

    const listRes = await request.get("/api/admin/emojis", { headers: auth });
    const emojis = (await listRes.json()) as { id: string; shortcode: string; category: string | null; tags: string[] }[];
    const imported = emojis.find((e) => e.shortcode === good);
    expect(imported).toMatchObject({ category: "e2e", tags: ["e2e_alias"] });
    importedId = imported?.id ?? null;

    // 管理画面にも直近のインポート結果が出る。
    await seedAuth(page, adminToken);
    await page.goto("/admin/emojis");
    await expect(page.getByText("完了", { exact: true }).first()).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText("追加: 1 / スキップ: 1 / 失敗: 1 / 合計: 3")).toBeVisible();

    // エクスポート: チケット付きURLは Authorization ヘッダー無しで落とせる。
    const ticketRes = await request.post("/api/admin/emojis/export/ticket", { headers: auth });
    expect(ticketRes.ok(), await ticketRes.text()).toBeTruthy();
    const { url } = (await ticketRes.json()) as { url: string };
    const exportRes = await request.get(url);
    expect(exportRes.ok(), `${exportRes.status()}`).toBeTruthy();
    expect(exportRes.headers()["content-type"]).toBe("application/zip");
    const files = readZip(await exportRes.body());
    const exportedMeta = JSON.parse(files.get("meta.json")!.toString("utf8")) as {
      metaVersion: number;
      emojis: { fileName: string; downloaded: boolean; emoji: { name: string; category: string; aliases: string[] } }[];
    };
    expect(exportedMeta.metaVersion).toBe(2);
    const entry = exportedMeta.emojis.find((e) => e.emoji.name === good);
    expect(entry).toMatchObject({ downloaded: true, emoji: { category: "e2e", aliases: ["e2e_alias"] } });
    expect(files.get(entry!.fileName)?.length ?? 0).toBeGreaterThan(0);

    // チケットが無効なら拒否、絵文字管理権限の無いユーザーにはチケットを出さない。
    expect((await request.get("/api/admin/emojis/export?ticket=invalid")).status()).toBe(401);
    const user = await registerUserViaApi(request, "e2eemojizip");
    const deniedTicket = await request.post("/api/admin/emojis/export/ticket", {
      headers: { Authorization: `Bearer ${user.token}` },
    });
    expect(deniedTicket.status()).toBe(403);
  } finally {
    if (importedId) {
      await request.delete(`/api/admin/emojis/${importedId}`, { headers: auth });
    }
    if (providerId) {
      await request.patch(`/api/admin/storage-providers/${providerId}`, {
        headers: auth,
        data: { is_active: false },
      });
    }
    await s3.close();
  }
});
