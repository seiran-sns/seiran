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

  return res.json() as Promise<T>;
}

// =====================================================================
// 型定義
// =====================================================================

export interface User {
  id: number;
  username: string;
  email: string;
  role: string; // "user" | "moderator" | "admin"
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

export interface AuthResponse {
  token: string;
  user: User;
}

export interface NoteAttachment {
  url: string;
  mimeType: string;
  width: number;
  height: number;
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
}

export interface ReactionSummary {
  emoji: string;
  count: number;
}

export interface ProfileNote {
  id: string;
  text: string;
  created_at: string;
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
  follow_status: "not_following" | "pending" | "accepted";
  recent_posts: ProfileNote[];
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
  blurhash: string;
  width: number;
  height: number;
  size: number;
  mimeType: string;
  isReused: boolean;
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
      replyToId?: string
    ) {
      return normalizeNote(
        await request<RawNote>("POST", "/notes/create", {
          text,
          deliver_to_fedi: deliverToFedi,
          deliver_to_bsky: deliverToBsky,
          attachment_ids: attachmentIds.length > 0 ? attachmentIds : undefined,
          reply_to_id: replyToId,
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

  users: {
    profile(q: string) {
      return request<UserProfile>("GET", `/users/profile?q=${encodeURIComponent(q)}`);
    },
    updateProfile(patch: {
      display_name?: string;
      bio?: string;
      avatar_media_id?: number | null;
      banner_media_id?: number | null;
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
      return request<{ ok: boolean }>("POST", `/admin/users/${encodeURIComponent(id)}/suspend`);
    },
    unsuspendUser(id: string) {
      return request<{ ok: boolean }>("POST", `/admin/users/${encodeURIComponent(id)}/unsuspend`);
    },
    changeUserRole(id: string, role: string) {
      return request<{ ok: boolean }>("POST", `/admin/users/${encodeURIComponent(id)}/role`, { role });
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
      return request<{ ok: boolean }>("DELETE", `/admin/storage-providers/${id}`);
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
      return request<{ ok: boolean }>("DELETE", `/admin/emojis/${encodeURIComponent(id)}`);
    },
  },

  follows: {
    create(target: string) {
      return request<FollowResponse>("POST", "/follows/create", { target });
    },
  },

  media: {
    upload(file: File, mediaType: "post" | "emoji" | "avatar" | "banner" = "post"): Promise<DriveFile> {
      const formData = new FormData();
      formData.append("file", file);
      formData.append("media_type", mediaType);
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
