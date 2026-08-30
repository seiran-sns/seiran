import { cursorParams, request } from "./core";
import type { BskyEmbedChoice, FrequentReaction, Note, NotificationItem, PollCreateInput, RawNote, ReactResult, ReactionActor, RepostEntry } from "./types";
import { normalizeNote } from "./types";

export const notes = {
  async get(id: string) {
    return normalizeNote(
      await request<RawNote>("GET", `/notes/${encodeURIComponent(id)}`),
    );
  },
  async create(
    text: string,
    deliverToFedi: boolean = true,
    deliverToBsky: boolean = true,
    attachmentIds: string[] = [],
    replyToId?: string,
    renoteId?: string,
    visibility?: "public" | "unlisted" | "followers_only" | "direct",
    recipientActorIds?: string[],
    quoteOfId?: string,
    bskyEmbedChoice?: BskyEmbedChoice,
    poll?: PollCreateInput,
    contentWarning?: string,
    linkCardUrls?: string[],
    language?: string,
  ) {
    return normalizeNote(
      await request<RawNote>("POST", "/notes/create", {
        text,
        deliver_to_fedi: deliverToFedi,
        deliver_to_bsky: deliverToBsky,
        attachment_ids: attachmentIds.length > 0 ? attachmentIds : undefined,
        reply_to_id: replyToId,
        renote_id: renoteId,
        quote_of_id: quoteOfId,
        visibility,
        recipient_actor_ids: recipientActorIds,
        bsky_embed_choice: bskyEmbedChoice,
        poll: poll && {
          choices: poll.choices,
          multiple: poll.multiple,
          expiresAtIso: poll.expiresAtIso,
          expiresInSeconds: poll.expiresInSeconds,
        },
        content_warning: contentWarning,
        link_card_urls: linkCardUrls && linkCardUrls.length > 0 ? linkCardUrls : undefined,
        language,
      }),
    );
  },
  async localTimeline(params?: {
    limit?: number;
    until_id?: string;
    since_id?: string;
    exclude_direct?: boolean;
  }) {
    const q = cursorParams(params);
    if (params?.exclude_direct) q.set("exclude_direct", "true");
    const qs = q.toString();
    const rows = await request<RawNote[]>(
      "GET",
      `/notes/local-timeline${qs ? `?${qs}` : ""}`,
    );
    return rows.map(normalizeNote);
  },
  async homeTimeline(params?: {
    limit?: number;
    until_id?: string;
    since_id?: string;
    exclude_direct?: boolean;
  }) {
    const q = cursorParams(params);
    if (params?.exclude_direct) q.set("exclude_direct", "true");
    const qs = q.toString();
    const rows = await request<RawNote[]>(
      "GET",
      `/notes/home-timeline${qs ? `?${qs}` : ""}`,
    );
    return rows.map(normalizeNote);
  },
  /** ソーシャルタイムライン（自分 + フォロー中 + ローカル全体、#78）。 */
  async socialTimeline(params?: {
    limit?: number;
    until_id?: string;
    since_id?: string;
    exclude_direct?: boolean;
  }) {
    const q = cursorParams(params);
    if (params?.exclude_direct) q.set("exclude_direct", "true");
    const qs = q.toString();
    const rows = await request<RawNote[]>(
      "GET",
      `/notes/social-timeline${qs ? `?${qs}` : ""}`,
    );
    return rows.map(normalizeNote);
  },
  /** グローバルタイムライン（postsテーブルの全投稿、#78）。 */
  async globalTimeline(params?: {
    limit?: number;
    until_id?: string;
    since_id?: string;
    exclude_direct?: boolean;
  }) {
    const q = cursorParams(params);
    if (params?.exclude_direct) q.set("exclude_direct", "true");
    const qs = q.toString();
    const rows = await request<RawNote[]>(
      "GET",
      `/notes/global-timeline${qs ? `?${qs}` : ""}`,
    );
    return rows.map(normalizeNote);
  },
  /** 前後のポスト（#226、最大5件＋読み込みボタン）。`beforeId`/`afterId`を渡すと
   * そのIDを起点に続きを取得する（省略時は対象ポスト自身が起点＝初回読み込み）。
   * `beforeLimit`/`afterLimit`を0にするとその方向は取得しない（片方向のみの読み込みボタン用）。 */
  async context(
    id: string,
    opts?: { beforeId?: string; afterId?: string; beforeLimit?: number; afterLimit?: number },
  ): Promise<{ before: Note[]; after: Note[] }> {
    const q = new URLSearchParams();
    if (opts?.beforeId) q.set("before_id", opts.beforeId);
    if (opts?.afterId) q.set("after_id", opts.afterId);
    if (opts?.beforeLimit !== undefined) q.set("before_limit", String(opts.beforeLimit));
    if (opts?.afterLimit !== undefined) q.set("after_limit", String(opts.afterLimit));
    const qs = q.toString();
    const raw = await request<{ before: RawNote[]; after: RawNote[] }>(
      "GET",
      `/notes/${encodeURIComponent(id)}/context${qs ? `?${qs}` : ""}`,
    );
    return {
      before: raw.before.map(normalizeNote),
      after: raw.after.map(normalizeNote),
    };
  },
  /** 対象ノートへの直系リプライ・引用の再帰取得（#226 返信タブ）。フラット配列で返る。 */
  async replies(id: string): Promise<Note[]> {
    const raw = await request<{ notes: RawNote[] }>(
      "GET",
      `/notes/${encodeURIComponent(id)}/replies`,
    );
    return raw.notes.map(normalizeNote);
  },
  /** 対象ノートへのリポスト一覧（#226 リポストタブ）。取り消し済みも履歴として含む。 */
  async reposts(id: string): Promise<RepostEntry[]> {
    return (
      await request<{ reposts: RepostEntry[] }>(
        "GET",
        `/notes/${encodeURIComponent(id)}/reposts`,
      )
    ).reposts;
  },
  deleteRepost(noteId: string) {
    return request<{ ok: boolean }>(
      "DELETE",
      `/notes/${encodeURIComponent(noteId)}/repost`,
    );
  },
  delete(noteId: string) {
    return request<{ ok: boolean }>(
      "DELETE",
      `/notes/${encodeURIComponent(noteId)}`,
    );
  },
  react(noteId: string, content: string) {
    return request<ReactResult>(
      "POST",
      `/notes/${encodeURIComponent(noteId)}/reactions`,
      { content },
    );
  },
  unreact(noteId: string, content: string) {
    return request<ReactResult>(
      "DELETE",
      `/notes/${encodeURIComponent(noteId)}/reactions/${encodeURIComponent(content)}`,
    );
  },
  reactionActors(noteId: string, content: string) {
    return request<{ actors: ReactionActor[] }>(
      "GET",
      `/notes/${encodeURIComponent(noteId)}/reactions/${encodeURIComponent(content)}/actors`,
    );
  },
  votePoll(noteId: string, optionIndexes: number[]) {
    return request<{
      ok: boolean;
      poll: NonNullable<Note["poll"]>;
      voted: boolean;
    }>("POST", `/notes/${encodeURIComponent(noteId)}/poll-vote`, {
      optionIndexes,
    });
  },
  pin(noteId: string) {
    return request<{ ok: boolean; pinnedPostIds: string[] }>(
      "POST",
      `/notes/${encodeURIComponent(noteId)}/pin`,
    );
  },
  unpin(noteId: string) {
    return request<{ ok: boolean; pinnedPostIds: string[] }>(
      "DELETE",
      `/notes/${encodeURIComponent(noteId)}/pin`,
    );
  },
  /** pendingなリプライ/引用/リポスト参照をその場で取り込む（#233/#234）。 */
  resolveReference(noteId: string, kind: "reply" | "quote" | "repost") {
    return request<{ status: "resolved" | "pending" | "gone" | "none"; postId: string | null }>(
      "POST",
      `/notes/${encodeURIComponent(noteId)}/resolve-reference`,
      { kind },
    );
  },
  async search(
    params: {
      q: string;
      limit?: number;
      session_id?: string;
      until_id?: string;
      since_id?: string;
    },
    signal?: AbortSignal,
  ) {
    const qs = new URLSearchParams();
    qs.set("q", params.q);
    if (params.limit) qs.set("limit", String(params.limit));
    if (params.session_id) qs.set("session_id", params.session_id);
    if (params.until_id) qs.set("until_id", params.until_id);
    if (params.since_id) qs.set("since_id", params.since_id);
    const raw = await request<{ notes: RawNote[]; session_id?: string }>(
      "GET",
      `/notes/search?${qs.toString()}`,
      undefined,
      signal,
    );
    return {
      notes: raw.notes.map(normalizeNote),
      session_id: raw.session_id,
    };
  },
};

/** Misskey API 互換の `/api/i/notifications`（Doc3 §5.5）。 */
export const notifications = {
  list(params?: {
    limit?: number;
    untilId?: string;
    sinceId?: string;
    markAsRead?: boolean;
  }) {
    return request<NotificationItem[]>("POST", "/i/notifications", {
      limit: params?.limit,
      untilId: params?.untilId,
      sinceId: params?.sinceId,
      markAsRead: params?.markAsRead,
    });
  },
};

export const reactions = {
  /** 自分がよく使う絵文字を頻度順に返す（絵文字ピッカーの「よく使う」タブ用）。 */
  frequent() {
    return request<{ items: FrequentReaction[] }>(
      "GET",
      "/reactions/frequent",
    );
  },
};
