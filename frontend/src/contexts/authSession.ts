import { ApiError, User } from "../api/client";

/**
 * 起動時のセッション確認（`/auth/me`）が失敗したときのリトライ間隔（#108）。
 * バックエンドの再起動・一時的な通信断は数秒で復旧することが多いため、
 * 明示的な401（認証失効）でない限りは少し待って再試行し、
 * ログイン状態を保ったまま復旧を待つ。
 */
export const AUTH_ME_RETRY_DELAYS_MS = [1000, 2000, 4000];

export type SessionResolution =
  | { kind: "authenticated"; user: User }
  | { kind: "expired" }
  | { kind: "unresolved" };

/**
 * `fetchMe` を401（明示的な認証失効）を除いて `retryDelays` 分だけリトライする。
 * バックエンド再起動中の接続失敗や5xxは認証失効ではないため `expired` にせず、
 * リトライしても解決しなければ `unresolved`（トークンは保持したまま）を返す。
 * 呼び出し側（`AuthProvider`）はこの結果に応じてトークンの破棄可否を判断する。
 */
export async function resolveSession(
  fetchMe: () => Promise<User>,
  retryDelays: readonly number[] = AUTH_ME_RETRY_DELAYS_MS,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
): Promise<SessionResolution> {
  for (let attempt = 0; ; attempt++) {
    try {
      const user = await fetchMe();
      return { kind: "authenticated", user };
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        return { kind: "expired" };
      }
      if (attempt >= retryDelays.length) {
        return { kind: "unresolved" };
      }
      await sleep(retryDelays[attempt]);
    }
  }
}
