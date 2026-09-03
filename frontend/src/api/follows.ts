import { cursorParams, request } from "./core";
import type { DmSession, FollowImportStartResponse, FollowImportStatusResponse, FollowListItem, FollowResponse, RawNote } from "./types";
import { normalizeNote } from "./types";

export const follows = {
  create(target: string) {
    return request<FollowResponse>("POST", "/follows/create", { target });
  },
  delete(target: string) {
    return request<void>("POST", "/follows/delete", { target });
  },
};

/** 承認制フォロー（鍵アカウント）の「承認待ちフォロー」画面（設定画面から遷移）用。 */
export const followRequests = {
  list() {
    return request<FollowListItem[]>("GET", "/follow-requests");
  },
  /** 設定アイコン・メニュー項目のバッジ表示用件数。 */
  count() {
    return request<{ count: number }>("GET", "/follow-requests/count");
  },
  accept(followerActorId: string) {
    return request<void>("POST", `/follow-requests/${followerActorId}/accept`);
  },
  reject(followerActorId: string) {
    return request<void>("POST", `/follow-requests/${followerActorId}/reject`);
  },
};

/** フォローインポート（設定画面から改行区切りのID一覧を貼り付けて一括フォロー）。
 * カンマ区切り1列目抽出（Misskeyエクスポート対応の隠し仕様）はバックエンド側で行うため、
 * ここでは textarea の生テキストをそのまま送るだけでよい。 */
export const followImport = {
  start(text: string) {
    return request<FollowImportStartResponse>("POST", "/account/follow-import", { text });
  },
  status() {
    return request<FollowImportStatusResponse>("GET", "/account/follow-import");
  },
  cancel() {
    return request<{ status: string }>("POST", "/account/follow-import/cancel");
  },
};

export const dm = {
  async sessions(params?: {
    limit?: number;
    until_id?: string;
    since_id?: string;
  }) {
    const qs = cursorParams(params).toString();
    return request<DmSession[]>("GET", `/dm/sessions${qs ? `?${qs}` : ""}`);
  },
  async threadMessages(
    threadRootId: string,
    params?: { limit?: number; until_id?: string; since_id?: string },
  ) {
    const qs = cursorParams(params).toString();
    const rows = await request<RawNote[]>(
      "GET",
      `/dm/sessions/${encodeURIComponent(threadRootId)}/messages${qs ? `?${qs}` : ""}`,
    );
    return rows.map(normalizeNote);
  },
  markRead(threadRootId: string) {
    return request<{ ok: boolean }>(
      "POST",
      `/dm/sessions/${encodeURIComponent(threadRootId)}/read`,
    );
  },
  unreadCount() {
    return request<{ count: number }>("GET", "/dm/unread-count");
  },
};
