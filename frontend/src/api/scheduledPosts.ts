import { request } from "./core";
import type { CreateNoteBody, RawNote, RawScheduledPost, ScheduledPost } from "./types";
import { normalizeNote } from "./types";

function normalizeScheduledPost(raw: RawScheduledPost): ScheduledPost {
  return {
    ...raw,
    replyTo: raw.replyTo ? normalizeNote(raw.replyTo) : null,
    quoteOf: raw.quoteOf ? normalizeNote(raw.quoteOf) : null,
  };
}

/** 予約投稿（機能 > 予約投稿、投稿欄の🕒）。 */
export const scheduledPosts = {
  async list() {
    const rows = await request<RawScheduledPost[]>("GET", "/scheduled-posts");
    return rows.map(normalizeScheduledPost);
  },
  /** メニューのバッジ用（投稿待ちは🕒、投稿できなかったものは件数バッジ）。 */
  counts() {
    return request<{ pending: number; failed: number }>("GET", "/scheduled-posts/counts");
  },
  async create(scheduledAt: string, body: CreateNoteBody) {
    return normalizeScheduledPost(
      await request<RawScheduledPost>("POST", "/scheduled-posts", { ...body, scheduled_at: scheduledAt }),
    );
  },
  /** 内容と予約日時を書き換えて投稿待ちに戻す（投稿できなかった予約の再予約を兼ねる）。 */
  async update(id: string, scheduledAt: string, body: CreateNoteBody) {
    return normalizeScheduledPost(
      await request<RawScheduledPost>("PUT", `/scheduled-posts/${encodeURIComponent(id)}`, {
        ...body,
        scheduled_at: scheduledAt,
      }),
    );
  },
  delete(id: string) {
    return request<void>("DELETE", `/scheduled-posts/${encodeURIComponent(id)}`);
  },
  /** 予約を（渡した内容で）今すぐ投稿する。 */
  async publish(id: string, body: CreateNoteBody) {
    return normalizeNote(
      await request<RawNote>("POST", `/scheduled-posts/${encodeURIComponent(id)}/publish`, body),
    );
  },
};
