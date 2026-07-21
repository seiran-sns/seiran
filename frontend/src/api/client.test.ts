import { beforeAll, describe, expect, it } from "vitest";
import i18n from "../i18n";
import { ApiError, getErrorMessage } from "./client";

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
    expect(getErrorMessage(err)).toBe("サーバーが応答していません。しばらく待ってから再試行してください");
  });

  it("未知のエラーコード・非5xxはUNKNOWN_WITH_CODEにフォールバックする", () => {
    const err = new ApiError("SOME_UNKNOWN_CODE", 418);
    expect(getErrorMessage(err)).toContain("SOME_UNKNOWN_CODE");
  });

  it("TypeError（fetch失敗・オフライン等）はNETWORK_ERRORになる", () => {
    expect(getErrorMessage(new TypeError("Failed to fetch"))).toBe(
      "サーバーに接続できません。ネットワーク接続を確認してください"
    );
  });

  it("ApiError/TypeError以外のErrorはmessageをそのまま返す", () => {
    expect(getErrorMessage(new Error("何か予期せぬ例外"))).toBe("何か予期せぬ例外");
  });

  it("Errorですらない値はUNKNOWNになる", () => {
    expect(getErrorMessage("just a string")).toBe("不明なエラーが発生しました");
    expect(getErrorMessage(undefined)).toBe("不明なエラーが発生しました");
  });
});
