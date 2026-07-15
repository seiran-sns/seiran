const BASE = "/api";

function getToken(): string | null {
  return localStorage.getItem("seiran_token");
}

function authHeaders(): Record<string, string> {
  const token = getToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

// =====================================================================
// 構造化エラー
// API は {"code": "...", "detail": {...}} の JSON を返す。
// フロントエンドが code を見てユーザー向けメッセージに変換する責務を持つ。
// =====================================================================

export class ApiError extends Error {
  constructor(
    public readonly code: string,
    public readonly status: number,
    public readonly detail?: Record<string, unknown>
  ) {
    super(code);
    this.name = "ApiError";
  }
}

const ERROR_MESSAGES: Record<string, string> = {
  EMAIL_ALREADY_REGISTERED: "このメールアドレスはすでに使用されています",
  EMAIL_INVALID: "有効なメールアドレスを入力してください",
  INVALID_TOKEN: "リンクが無効か期限切れです。メールの確認をやり直してください",
  USERNAME_TAKEN: "このユーザー名はすでに使用されています",
  INVALID_INPUT: "入力内容を確認してください",
  ALREADY_INITIALIZED: "セットアップはすでに完了しています",
  INVALID_CREDENTIALS: "メールアドレスまたはパスワードが正しくありません",
  REGISTRATION_TOKEN_INVALID: "メール確認をやり直してください",
  UNAUTHORIZED: "ログインが必要です",
  NOT_FOUND: "見つかりません",
  INTERNAL_ERROR: "サーバーエラーが発生しました。しばらく待ってから再試行してください",
  RESET_TOKEN_INVALID: "リンクが無効または期限切れです。パスワードリセットをやり直してください",
  PASSWORD_TOO_SHORT: "パスワードは8文字以上で入力してください",
  TEXT_TOO_LONG: "文字数制限を超えています（@ユーザー名の展開後に超過した可能性があります）",
};

export function getErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    return ERROR_MESSAGES[error.code] ?? `エラーが発生しました (${error.code})`;
  }
  if (error instanceof Error) return error.message;
  return "不明なエラーが発生しました";
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal
): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...authHeaders(),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
    signal,
  });

  if (!res.ok) {
    const contentType = res.headers.get("content-type") ?? "";
    if (contentType.includes("application/json")) {
      try {
        const err = (await res.json()) as { code?: string; detail?: Record<string, unknown> };
        if (err.code) {
          throw new ApiError(err.code, res.status, err.detail);
        }
      } catch (e) {
        if (e instanceof ApiError) throw e;
      }
    }
    throw new ApiError("UNKNOWN_ERROR", res.status);
  }

  // 204 No Content 等、ボディが無い成功レスポンスは res.json() が
  // "Unexpected end of JSON input" で例外を投げるため、パース前に弾く
  // （例: admin のロール変更/凍結・解除 API。処理自体は成功しているのに
  // 呼び出し側にエラーとして伝播していた不具合）。
  if (res.status === 204) {
    return undefined as T;
  }
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

// =====================================================================
// 型定義
// =====================================================================

export interface User {
  id: number;
  username: string;
  email: string;
  role: string; // "user" | "moderator" | "admin"
  /** ローカル actors.id。noteUpdated ストリームイベントの reactorActorId との突き合わせに使う。 */
  actor_id: number;
  /** 左下ナビ等の自分のアイコン表示用。未設定の場合は undefined。 */
  avatar_url?: string;
}

// ── 管理画面用の型（レスポンスは snake_case） ──────────────────────────────

export interface AdminUser {
  id: string;
  email: string;
  role: string;
  suspended_at: string | null;
  username: string | null;
}

export interface StorageProvider {
  id: number;
  name: string;
  endpoint: string;
  bucket: string;
  region: string;
  access_key: string;
  secret_key_set: boolean;
  public_url: string;
  capacity_mb: number | null;
  is_active: boolean;
  created_at: string;
}

export interface SiteSettings {
  smtp_host: string;
  smtp_port: string;
  smtp_username: string;
  smtp_password_set: boolean;
  smtp_from: string;
  require_email_verification: string;
  site_name: string;
  site_color: string;
  site_icon_url: string;
}

export interface CustomEmoji {
  id: string;
  shortcode: string;
  media_file_id: string;
  category: string | null;
  /** タグ（#49）。ピッカーの部分一致対象。 */
  tags: string[];
  created_at: string;
}

export interface EmojiImportJob {
  jobId: string;
  total: number;
  processed: number;
  skipped: number;
  failed: number;
  done: boolean;
  errors: string[];
}

export interface AuthResponse {
  token: string;
  user: User;
}

export interface NoteAttachment {
  url: string;
  mimeType: string;
  width: number;
  height: number;
  thumbnailUrl?: string;
  durationMs?: number;
}

/** NoteResponse（バックエンドは `#[serde(rename_all = "camelCase")]`）。 */
export interface Note {
  id: string;
  text: string;
  createdAt: string;
  user: {
    id: number;
    username: string;
    domain?: string;
    displayName?: string;
    actorType: string; // "local" | "fedi" | "bsky" | "remote_seiran" | ...
    avatarUrl?: string;
  };
  attachments: NoteAttachment[];
  // 7.2 拡張メタデータ（存在する場合のみ）
  renoteId?: string;
  quoteId?: string;
  replyId?: string;
  parentOriginalId?: string;
  // リアクション集計（#22）
  reactions?: ReactionSummary[];
  /** リポストの場合の元ポスト実体（#45）。この Note 自身は「リポストした」ラッパ。 */
  renote?: Note;
  /** 認証ユーザーがこのノートをリポスト済みかどうか（未認証時は undefined）。 */
  repostedByMe?: boolean;
  /** 本文・投稿者表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（Fedi受信のみ）。 */
  emojis?: Record<string, string>;
  /** 認証ユーザー自身の投稿がピン留め済みかどうか（#61）。自分のプロフィール表示時のみ設定。 */
  pinnedByMe?: boolean;
}

export interface ReactionSummary {
  emoji: string;
  count: number;
  reactedByMe: boolean;
  /** Fedi から受信したカスタム絵文字（`:shortcode:`）の画像URL。Unicode絵文字は undefined。 */
  emojiUrl?: string;
}

export interface ReactResult {
  ok: boolean;
  reactions: ReactionSummary[];
}

/** ProfileResponse（バックエンドは snake_case のまま）。 */
export interface UserProfile {
  username: string;
  domain: string;
  display_name?: string;
  actor_type: string;
  ap_uri?: string;
  at_did?: string;
  bio?: string;
  avatar_url?: string;
  follow_status: "not_following" | "pending" | "accepted";
  /** 最近の投稿。タイムラインと同じ NoteCard で描画する（#43）。 */
  recent_posts: Note[];
  /** ピン留め投稿（#61）。ローカルユーザーの pin/unpin 操作結果、またはリモートアクターの
   * Fedi featured collection / Bsky pinnedPost の同期結果。 */
  pinned_posts: Note[];
  // 7.3 ブリッジ介入・魂の結合メタデータ
  bridge_real_handle?: string;
  bridge_protocol?: string; // "fedi" | "bsky"
  is_paired: boolean;
}

export interface SearchResult {
  notes: Note[];
  session_id?: string;
}

/**
 * バックエンドの生レスポンス。NoteResponse は camelCase 化の移行途中で、
 * 稼働中バイナリの世代によって snake_case（`created_at`）を返す場合があるため、
 * 両方のキーを許容してフロント内部では camelCase の `Note` に正規化する。
 */
interface RawNote {
  id: string | number;
  text?: string;
  createdAt?: string;
  created_at?: string;
  user?: {
    id: number;
    username: string;
    domain?: string;
    displayName?: string;
    display_name?: string;
    actorType?: string;
    actor_type?: string;
    avatarUrl?: string;
    avatar_url?: string;
  };
  attachments?: NoteAttachment[];
  renoteId?: string;
  renote_id?: string;
  quoteId?: string;
  quote_id?: string;
  replyId?: string;
  reply_id?: string;
  parentOriginalId?: string;
  parent_original_id?: string;
  reactions?: ReactionSummary[];
  renote?: RawNote;
  repostedByMe?: boolean;
  reposted_by_me?: boolean;
  emojis?: Record<string, string>;
  pinnedByMe?: boolean;
  pinned_by_me?: boolean;
}

/** snake_case / camelCase 混在に耐えるノート正規化。 */
function normalizeNote(r: RawNote): Note {
  return {
    id: String(r.id),
    text: r.text ?? "",
    createdAt: r.createdAt ?? r.created_at ?? "",
    user: {
      id: r.user?.id ?? 0,
      username: r.user?.username ?? "",
      domain: r.user?.domain,
      displayName: r.user?.displayName ?? r.user?.display_name,
      actorType: r.user?.actorType ?? r.user?.actor_type ?? "local",
      avatarUrl: r.user?.avatarUrl ?? r.user?.avatar_url,
    },
    attachments: r.attachments ?? [],
    renoteId: r.renoteId ?? r.renote_id,
    quoteId: r.quoteId ?? r.quote_id,
    replyId: r.replyId ?? r.reply_id,
    parentOriginalId: r.parentOriginalId ?? r.parent_original_id,
    reactions: r.reactions ?? [],
    renote: r.renote ? normalizeNote(r.renote) : undefined,
    repostedByMe: r.repostedByMe ?? r.reposted_by_me,
    emojis: r.emojis,
    pinnedByMe: r.pinnedByMe ?? r.pinned_by_me,
  };
}

/** ストリーミング（#37）で受け取った note ペイロードを Note に正規化する。 */
export function noteFromStream(body: unknown): Note {
  return normalizeNote(body as RawNote);
}

export interface FollowResponse {
  status: string;
  target_uri: string;
}

export interface DriveFile {
  id: string;
  url: string;
  sha256: string;
  blurhash?: string;
  width?: number;
  height?: number;
  size: number;
  mimeType: string;
  isReused: boolean;
  durationMs?: number;
  thumbnailUrl?: string;
}

/**
 * `POST /api/i/notifications`（Misskey API 互換）のレスポンス要素。
 * バックエンドは既に camelCase で返すため正規化不要（`Note`/`RawNote` と違い
 * snake_case な旧世代レスポンスとの互換を持たない新規エンドポイントのため）。
 */
export interface NotificationUser {
  id: string;
  username: string;
  /** ローカルユーザーは null。 */
  host: string | null;
  name?: string;
  avatarUrl?: string;
}

export interface NotificationItem {
  id: string;
  createdAt: string;
  type: string; // "reaction" | "follow" | "followRequestAccepted"
  userId?: string;
  user?: NotificationUser;
  /** `type === "reaction"` の場合のみ。カスタム絵文字は `:shortcode:` 形式。 */
  reaction?: string;
  /**
   * `type === "reaction"` の場合のみ。`note.reactionEmojis` にカスタム絵文字（`reaction` と
   * 同じキー）の画像URLが入っている場合のみ画像表示する（Unicode絵文字は入らない）。
   */
  note?: { reactionEmojis?: Record<string, string> };
}

// =====================================================================
// Auth API
// =====================================================================

export interface VerifyEmailResponse {
  message: string;
}

export interface VerifyTokenResponse {
  registration_token: string;
}

export interface SetupStatus {
  initialized: boolean;
}

export interface MetaResponse {
  uri: string;
  name: string;
  version: string;
  features: {
    registration: boolean;
    miauth: boolean;
  };
  requireEmailVerification: boolean;
  siteColor?: string;
  siteIconUrl?: string;
}

export const api = {
  meta(signal?: AbortSignal) {
    return request<MetaResponse>("POST", "/meta", undefined, signal);
  },

  setup: {
    status(signal?: AbortSignal) {
      return request<SetupStatus>("GET", "/setup/status", undefined, signal);
    },
    initialize(username: string, email: string, password: string) {
      return request<AuthResponse>("POST", "/setup", { username, email, password });
    },
  },

  auth: {
    requestEmailVerification(email: string) {
      return request<VerifyEmailResponse>("POST", "/auth/verify-email", { email });
    },
    verifyEmailToken(token: string, signal?: AbortSignal) {
      return request<VerifyTokenResponse>("GET", `/auth/verify-token?token=${encodeURIComponent(token)}`, undefined, signal);
    },
    register(username: string, password: string, registrationToken: string) {
      return request<AuthResponse>("POST", "/auth/register", {
        username,
        password,
        registration_token: registrationToken,
      });
    },
    registerDirect(email: string, username: string, password: string) {
      return request<AuthResponse>("POST", "/auth/register", {
        username,
        password,
        email,
      });
    },
    login(identifier: string, password: string) {
      return request<AuthResponse>("POST", "/auth/login", { identifier, password });
    },
    me() {
      return request<User>("GET", "/auth/me");
    },
    requestPasswordReset(email: string) {
      return request<{ message: string }>("POST", "/auth/request-password-reset", { email });
    },
    verifyResetToken(token: string, signal?: AbortSignal) {
      return request<{ valid: boolean }>("GET", `/auth/verify-reset-token?token=${encodeURIComponent(token)}`, undefined, signal);
    },
    resetPassword(token: string, newPassword: string) {
      return request<{ message: string }>("POST", "/auth/reset-password", { token, new_password: newPassword });
    },
  },

  notes: {
    async get(id: string) {
      return normalizeNote(await request<RawNote>("GET", `/notes/${encodeURIComponent(id)}`));
    },
    async create(
      text: string,
      deliverToFedi: boolean = true,
      deliverToBsky: boolean = true,
      attachmentIds: string[] = [],
      replyToId?: string,
      renoteId?: string
    ) {
      return normalizeNote(
        await request<RawNote>("POST", "/notes/create", {
          text,
          deliver_to_fedi: deliverToFedi,
          deliver_to_bsky: deliverToBsky,
          attachment_ids: attachmentIds.length > 0 ? attachmentIds : undefined,
          reply_to_id: replyToId,
          renote_id: renoteId,
        })
      );
    },
    async localTimeline(params?: { limit?: number; until_id?: string; since_id?: string }) {
      const q = new URLSearchParams();
      if (params?.limit) q.set("limit", String(params.limit));
      if (params?.until_id) q.set("until_id", params.until_id);
      if (params?.since_id) q.set("since_id", params.since_id);
      const qs = q.toString();
      const rows = await request<RawNote[]>("GET", `/notes/local-timeline${qs ? `?${qs}` : ""}`);
      return rows.map(normalizeNote);
    },
    async homeTimeline(params?: { limit?: number; until_id?: string; since_id?: string }) {
      const q = new URLSearchParams();
      if (params?.limit) q.set("limit", String(params.limit));
      if (params?.until_id) q.set("until_id", params.until_id);
      if (params?.since_id) q.set("since_id", params.since_id);
      const qs = q.toString();
      const rows = await request<RawNote[]>("GET", `/notes/home-timeline${qs ? `?${qs}` : ""}`);
      return rows.map(normalizeNote);
    },
    async context(id: string): Promise<{ before: Note[]; after: Note[] }> {
      const raw = await request<{ before: RawNote[]; after: RawNote[] }>(
        "GET",
        `/notes/${encodeURIComponent(id)}/context`
      );
      return { before: raw.before.map(normalizeNote), after: raw.after.map(normalizeNote) };
    },
    deleteRepost(noteId: string) {
      return request<{ ok: boolean }>("DELETE", `/notes/${encodeURIComponent(noteId)}/repost`);
    },
    react(noteId: string, content: string) {
      return request<ReactResult>("POST", `/notes/${encodeURIComponent(noteId)}/reactions`, { content });
    },
    unreact(noteId: string, content: string) {
      return request<ReactResult>(
        "DELETE",
        `/notes/${encodeURIComponent(noteId)}/reactions/${encodeURIComponent(content)}`
      );
    },
    pin(noteId: string) {
      return request<{ ok: boolean; pinnedPostIds: string[] }>("POST", `/notes/${encodeURIComponent(noteId)}/pin`);
    },
    unpin(noteId: string) {
      return request<{ ok: boolean; pinnedPostIds: string[] }>("DELETE", `/notes/${encodeURIComponent(noteId)}/pin`);
    },
    async search(params: { q: string; limit?: number; session_id?: string }, signal?: AbortSignal) {
      const qs = new URLSearchParams();
      qs.set("q", params.q);
      if (params.limit) qs.set("limit", String(params.limit));
      if (params.session_id) qs.set("session_id", params.session_id);
      const raw = await request<{ notes: RawNote[]; session_id?: string }>(
        "GET",
        `/notes/search?${qs.toString()}`,
        undefined,
        signal
      );
      return { notes: raw.notes.map(normalizeNote), session_id: raw.session_id };
    },
  },

  /** Misskey API 互換の `/api/i/notifications`（Doc3 §5.5）。 */
  notifications: {
    list(params?: { limit?: number; untilId?: string; sinceId?: string; markAsRead?: boolean }) {
      return request<NotificationItem[]>("POST", "/i/notifications", {
        limit: params?.limit,
        untilId: params?.untilId,
        sinceId: params?.sinceId,
        markAsRead: params?.markAsRead,
      });
    },
  },

  users: {
    async profile(q: string) {
      const raw = await request<
        Omit<UserProfile, "recent_posts" | "pinned_posts"> & { recent_posts?: RawNote[]; pinned_posts?: RawNote[] }
      >("GET", `/users/profile?q=${encodeURIComponent(q)}`);
      // recent_posts / pinned_posts はタイムラインと同じ NoteCard で描画するため Note に正規化（#43, #61）。
      return {
        ...raw,
        recent_posts: (raw.recent_posts ?? []).map(normalizeNote),
        pinned_posts: (raw.pinned_posts ?? []).map(normalizeNote),
      } as UserProfile;
    },
    updateProfile(patch: {
      display_name?: string;
      bio?: string;
      avatar_media_id?: string | null;
      banner_media_id?: string | null;
    }) {
      return request<{
        username: string;
        display_name?: string;
        bio?: string;
        avatar_media_id?: number;
        banner_media_id?: number;
      }>("PATCH", "/users/profile", patch);
    },
  },

  admin: {
    listUsers() {
      return request<AdminUser[]>("GET", "/admin/users");
    },
    suspendUser(id: string) {
      return request<void>("POST", `/admin/users/${encodeURIComponent(id)}/suspend`);
    },
    unsuspendUser(id: string) {
      return request<void>("POST", `/admin/users/${encodeURIComponent(id)}/unsuspend`);
    },
    changeUserRole(id: string, role: string) {
      return request<void>("POST", `/admin/users/${encodeURIComponent(id)}/role`, { role });
    },

    getSiteSettings() {
      return request<SiteSettings>("GET", "/admin/site-settings");
    },
    updateSiteSettings(patch: Partial<{
      smtp_host: string;
      smtp_port: string;
      smtp_username: string;
      smtp_password: string;
      smtp_from: string;
      require_email_verification: string;
      site_name: string;
      site_color: string;
      site_icon_url: string;
    }>) {
      return request<SiteSettings>("PATCH", "/admin/site-settings", patch);
    },

    listStorageProviders() {
      return request<StorageProvider[]>("GET", "/admin/storage-providers");
    },
    createStorageProvider(body: {
      name: string;
      endpoint: string;
      bucket: string;
      region?: string;
      access_key: string;
      secret_key: string;
      public_url: string;
      capacity_mb?: number | null;
    }) {
      return request<StorageProvider>("POST", "/admin/storage-providers", body);
    },
    updateStorageProvider(id: number, patch: Record<string, unknown>) {
      return request<StorageProvider>("PATCH", `/admin/storage-providers/${id}`, patch);
    },
    deleteStorageProvider(id: number) {
      return request<void>("DELETE", `/admin/storage-providers/${id}`);
    },

    listEmojis() {
      return request<CustomEmoji[]>("GET", "/admin/emojis");
    },
    createEmoji(body: { shortcode: string; media_file_id: number; category?: string; tags?: string[] }) {
      return request<CustomEmoji>("POST", "/admin/emojis", body);
    },
    updateEmoji(id: string, body: { category?: string; tags?: string[] }) {
      return request<CustomEmoji>("PATCH", `/admin/emojis/${encodeURIComponent(id)}`, body);
    },
    deleteEmoji(id: string) {
      return request<void>("DELETE", `/admin/emojis/${encodeURIComponent(id)}`);
    },
    importEmojis(file: File): Promise<EmojiImportJob> {
      const formData = new FormData();
      formData.append("file", file);
      return fetch(`${BASE}/admin/emojis/import`, {
        method: "POST",
        headers: { ...authHeaders() },
        body: formData,
      }).then(async (res) => {
        if (!res.ok) {
          const contentType = res.headers.get("content-type") ?? "";
          if (contentType.includes("application/json")) {
            try {
              const err = (await res.json()) as { code?: string };
              if (err.code) throw new ApiError(err.code, res.status);
            } catch (e) {
              if (e instanceof ApiError) throw e;
            }
          }
          throw new ApiError("UNKNOWN_ERROR", res.status);
        }
        return res.json() as Promise<EmojiImportJob>;
      });
    },
    getEmojiImportStatus(jobId: string) {
      return request<EmojiImportJob>("GET", `/admin/emojis/import/${encodeURIComponent(jobId)}`);
    },
  },

  follows: {
    create(target: string) {
      return request<FollowResponse>("POST", "/follows/create", { target });
    },
    delete(target: string) {
      return request<void>("POST", "/follows/delete", { target });
    },
  },

  account: {
    withdraw(confirmHandle: string) {
      return request<void>("POST", "/account/withdraw", { confirm_handle: confirmHandle });
    },
  },

  miauth: {
    /** MiAuth 認可確認画面（`/connect/:sessionId`）で「承認する」を押した時に呼ぶ。 */
    authorize(sessionId: string) {
      return request<{ ok: boolean }>("POST", `/miauth/${encodeURIComponent(sessionId)}/authorize`);
    },
  },

  media: {
    /**
     * `deliverToBsky`: 動画添付のみ意味を持つ。Bluesky公式動画パイプラインへの
     * 提出可否（省略時true）。falseにすると音声・画像と同様、Bskyへは
     * externalリンクカードとして配信される。
     */
    upload(file: File, mediaType: "post" | "emoji" | "avatar" | "banner" = "post", deliverToBsky = true): Promise<DriveFile> {
      const formData = new FormData();
      formData.append("file", file);
      formData.append("media_type", mediaType);
      formData.append("deliver_to_bsky", String(deliverToBsky));
      return fetch(`${BASE}/drive/files/create`, {
        method: "POST",
        headers: { ...authHeaders() },
        body: formData,
      }).then(async (res) => {
        if (!res.ok) {
          const contentType = res.headers.get("content-type") ?? "";
          if (contentType.includes("application/json")) {
            try {
              const err = (await res.json()) as { code?: string; detail?: Record<string, unknown> };
              if (err.code) throw new ApiError(err.code, res.status, err.detail);
            } catch (e) {
              if (e instanceof ApiError) throw e;
            }
          }
          throw new ApiError("UNKNOWN_ERROR", res.status);
        }
        return res.json() as Promise<DriveFile>;
      });
    },
  },
};

export { getToken };
