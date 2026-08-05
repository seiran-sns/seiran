import { describe, expect, it, vi } from "vitest";
import { ApiError, User } from "../api/client";
import { resolveSession } from "./authSession";

function makeUser(): User {
  return {
    id: 1,
    username: "alice",
    email: "alice@example.com",
    role: "user",
    actor_id: 1,
    language_preference: null,
    token: "test-token",
  };
}

describe("resolveSession", () => {
  it("成功時はauthenticatedとuserを返す", async () => {
    const fetchMe = vi.fn().mockResolvedValue(makeUser());
    const result = await resolveSession(fetchMe, [1, 2], async () => {});
    expect(result).toEqual({ kind: "authenticated", user: makeUser() });
    expect(fetchMe).toHaveBeenCalledTimes(1);
  });

  it("401はリトライせず即座にexpiredを返す（#108: 明示的な認証失効のみログアウト対象）", async () => {
    const fetchMe = vi.fn().mockRejectedValue(new ApiError("UNAUTHORIZED", 401));
    const sleep = vi.fn().mockResolvedValue(undefined);
    const result = await resolveSession(fetchMe, [1, 2], sleep);
    expect(result).toEqual({ kind: "expired" });
    expect(fetchMe).toHaveBeenCalledTimes(1);
    expect(sleep).not.toHaveBeenCalled();
  });

  it(
    "ネットワークエラー等はリトライし、途中で成功すればauthenticatedを返す" +
      "（#108: バックエンド再起動中の接続断でログアウトさせない）",
    async () => {
      const fetchMe = vi
        .fn()
        .mockRejectedValueOnce(new TypeError("Failed to fetch"))
        .mockRejectedValueOnce(new ApiError("SERVER_UNAVAILABLE", 503))
        .mockResolvedValueOnce(makeUser());
      const sleep = vi.fn().mockResolvedValue(undefined);
      const result = await resolveSession(fetchMe, [1, 2, 3], sleep);
      expect(result).toEqual({ kind: "authenticated", user: makeUser() });
      expect(fetchMe).toHaveBeenCalledTimes(3);
      expect(sleep).toHaveBeenCalledTimes(2);
    }
  );

  it("リトライ回数を使い切っても解決しなければunresolvedを返す（トークンは破棄しない）", async () => {
    const fetchMe = vi.fn().mockRejectedValue(new TypeError("Failed to fetch"));
    const sleep = vi.fn().mockResolvedValue(undefined);
    const result = await resolveSession(fetchMe, [1, 2], sleep);
    expect(result).toEqual({ kind: "unresolved" });
    expect(fetchMe).toHaveBeenCalledTimes(3);
    expect(sleep).toHaveBeenCalledTimes(2);
  });
});
