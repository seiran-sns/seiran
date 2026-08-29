import { cursorParams, request } from "./core";
import type { ListDetail, ListMember, ListSummary, RawNote } from "./types";
import { normalizeNote } from "./types";

export const lists = {
  list() {
    return request<ListSummary[]>("GET", "/lists");
  },
  create(name: string, isPublic: boolean) {
    return request<ListSummary>("POST", "/lists", {
      name,
      is_public: isPublic,
    });
  },
  get(id: string) {
    return request<ListDetail>("GET", `/lists/${encodeURIComponent(id)}`);
  },
  update(id: string, name: string, isPublic: boolean) {
    return request<ListSummary>("PATCH", `/lists/${encodeURIComponent(id)}`, {
      name,
      is_public: isPublic,
    });
  },
  remove(id: string) {
    return request<void>("DELETE", `/lists/${encodeURIComponent(id)}`);
  },
  addMember(id: string, target: string) {
    return request<ListMember[]>(
      "POST",
      `/lists/${encodeURIComponent(id)}/members`,
      { target },
    );
  },
  removeMember(id: string, actorId: string) {
    return request<void>(
      "DELETE",
      `/lists/${encodeURIComponent(id)}/members/${encodeURIComponent(actorId)}`,
    );
  },
  async timeline(
    id: string,
    params?: { limit?: number; until_id?: string; since_id?: string },
  ) {
    const qs = cursorParams(params).toString();
    const rows = await request<RawNote[]>(
      "GET",
      `/lists/${encodeURIComponent(id)}/timeline${qs ? `?${qs}` : ""}`,
    );
    return rows.map(normalizeNote);
  },
};

export const hashtags = {
  /** ホーム画面タブにピン留めされたハッシュタグ一覧。 */
  pinned() {
    return request<{ name: string }[]>("GET", "/hashtags/pinned");
  },
  pin(name: string) {
    return request<void>("POST", `/hashtags/${encodeURIComponent(name)}/pin`);
  },
  unpin(name: string) {
    return request<void>(
      "DELETE",
      `/hashtags/${encodeURIComponent(name)}/pin`,
    );
  },
  async timeline(
    name: string,
    params?: { limit?: number; until_id?: string; since_id?: string },
  ) {
    const qs = cursorParams(params).toString();
    const rows = await request<RawNote[]>(
      "GET",
      `/hashtags/${encodeURIComponent(name)}/timeline${qs ? `?${qs}` : ""}`,
    );
    return rows.map(normalizeNote);
  },
};
