import { beforeAll, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import {
  ApiError,
  api,
  cursorParams,
  getErrorMessage,
  noteFromStream,
  parseJsonBody,
  setUnauthorizedHandler,
  throwIfError,
} from "./client";

// メッセージ文言はロケール依存のため、テストでは言語を固定してから検証する。
beforeAll(async () => {
  await i18n.changeLanguage("ja");
});

describe("getErrorMessage", () => {
  it("i18nに登録済みのエラーコードはそのメッセージを返す", () => {
    const err = new ApiError("ACTOR_NOT_FOUND", 404);
    expect(getErrorMessage(err)).toBe("アクターが見つかりません");
  });

  it("未知のエラーコード・5xxはSERVER_UNAVAILABLEにフォールバックする", () => {
    const err = new ApiError("SOME_UNKNOWN_CODE", 503);
    expect(getErrorMessage(err)).toBe(
      "サーバーが応答していません。しばらく待ってから再試行してください",
    );
  });

  it("未知のエラーコード・非5xxはUNKNOWN_WITH_CODEにフォールバックする", () => {
    const err = new ApiError("SOME_UNKNOWN_CODE", 418);
    expect(getErrorMessage(err)).toContain("SOME_UNKNOWN_CODE");
  });

  it("TypeError（fetch失敗・オフライン等）はNETWORK_ERRORになる", () => {
    expect(getErrorMessage(new TypeError("Failed to fetch"))).toBe(
      "サーバーに接続できません。ネットワーク接続を確認してください",
    );
  });

  it("ApiError/TypeError以外のErrorはmessageをそのまま返す", () => {
    expect(getErrorMessage(new Error("何か予期せぬ例外"))).toBe(
      "何か予期せぬ例外",
    );
  });

  it("Errorですらない値はUNKNOWNになる", () => {
    expect(getErrorMessage("just a string")).toBe("不明なエラーが発生しました");
    expect(getErrorMessage(undefined)).toBe("不明なエラーが発生しました");
  });
});

describe("cursorParams", () => {
  it("limit/until_id/since_idを全て指定した場合すべて含む", () => {
    const q = cursorParams({ limit: 30, until_id: "100", since_id: "200" });
    expect(q.get("limit")).toBe("30");
    expect(q.get("until_id")).toBe("100");
    expect(q.get("since_id")).toBe("200");
  });

  it("未指定のフィールドは含まない", () => {
    const q = cursorParams({ limit: 10 });
    expect(q.get("limit")).toBe("10");
    expect(q.has("until_id")).toBe(false);
    expect(q.has("since_id")).toBe(false);
  });

  it("paramsそのものがundefinedなら空のURLSearchParamsを返す", () => {
    expect(cursorParams(undefined).toString()).toBe("");
  });

  it("limit=0は指定なし扱いになる（falsy判定）", () => {
    // limit=0 は意味を持たない値のため、0 の場合は付与されない仕様。
    const q = cursorParams({ limit: 0 });
    expect(q.has("limit")).toBe(false);
  });
});

describe("parseJsonBody", () => {
  it("204 No Contentはundefinedを返す（res.json()を呼ばない）", async () => {
    const res = new Response(null, { status: 204 });
    expect(await parseJsonBody(res)).toBeUndefined();
  });

  it("空文字列ボディはundefinedを返す", async () => {
    const res = new Response("", { status: 200 });
    expect(await parseJsonBody(res)).toBeUndefined();
  });

  it("JSON文字列ボディはパースして返す", async () => {
    const res = new Response(JSON.stringify({ ok: true, count: 3 }), {
      status: 200,
    });
    expect(await parseJsonBody(res)).toEqual({ ok: true, count: 3 });
  });
});

describe("throwIfError", () => {
  it("/auth/meの401はグローバル認証失効ハンドラを呼ばない（#108）", async () => {
    const handler = vi.fn();
    setUnauthorizedHandler(handler);
    vi.stubGlobal("localStorage", {
      getItem: vi.fn().mockReturnValue("token"),
      setItem: vi.fn(),
      removeItem: vi.fn(),
      clear: vi.fn(),
      key: vi.fn(),
      length: 1,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ code: "UNAUTHORIZED" }), {
          status: 401,
          headers: { "content-type": "application/json" },
        }),
      ),
    );

    await expect(api.auth.me()).rejects.toMatchObject({
      code: "UNAUTHORIZED",
      status: 401,
    });
    expect(handler).not.toHaveBeenCalled();

    vi.unstubAllGlobals();
    setUnauthorizedHandler(null);
  });
  it("res.okがtrueなら何もしない", async () => {
    const res = new Response(null, { status: 200 });
    await expect(throwIfError(res)).resolves.toBeUndefined();
  });

  it("JSONエラーボディにcodeがあればApiErrorとしてそれを投げる", async () => {
    const res = new Response(
      JSON.stringify({ code: "NOT_FOUND", detail: { id: "1" } }),
      {
        status: 404,
        headers: { "content-type": "application/json" },
      },
    );
    await expect(throwIfError(res)).rejects.toMatchObject({
      code: "NOT_FOUND",
      status: 404,
    });
  });

  it("非JSONレスポンス（アップロードAPI等のエラー）はUNKNOWN_ERRORになる", async () => {
    const res = new Response("Internal Server Error", {
      status: 500,
      headers: { "content-type": "text/plain" },
    });
    await expect(throwIfError(res)).rejects.toMatchObject({
      code: "UNKNOWN_ERROR",
      status: 500,
    });
  });

  it("JSONだがcodeフィールドが無ければUNKNOWN_ERRORになる", async () => {
    const res = new Response(JSON.stringify({ message: "something" }), {
      status: 400,
      headers: { "content-type": "application/json" },
    });
    await expect(throwIfError(res)).rejects.toMatchObject({
      code: "UNKNOWN_ERROR",
      status: 400,
    });
  });
});

// #102: バックエンドは正しくcontentWarning/pollを返しているのに、正規化処理から
// この2フィールドが漏れていて常にundefinedになりCW・アンケートUIが表示されない回帰があった。
describe("noteFromStream（RawNote正規化）", () => {
  it("contentWarningとpollをNoteへコピーする", () => {
    const raw = {
      id: "1",
      text: "本文",
      user: { id: 1, username: "alice" },
      contentWarning: "注意書き",
      poll: { multiple: false, options: [{ name: "A", votes: 1 }] },
    };
    const note = noteFromStream(raw);
    expect(note.contentWarning).toBe("注意書き");
    expect(note.poll).toEqual({
      multiple: false,
      options: [{ name: "A", votes: 1 }],
    });
  });

  it("contentWarning/pollが無ければundefinedのまま", () => {
    const raw = { id: "1", text: "本文", user: { id: 1, username: "alice" } };
    const note = noteFromStream(raw);
    expect(note.contentWarning).toBeUndefined();
    expect(note.poll).toBeUndefined();
  });
});
