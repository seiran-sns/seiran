import i18n from "../i18n";

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

/**
 * バックエンドのエラーコード（`crates/seiran-api/src/error.rs`）を
 * `errors.*`（`frontend/src/i18n/locales/{lng}/errors.json`）へ機械的に対応させる。
 * 未知のコードは 5xx なら SERVER_UNAVAILABLE、それ以外は UNKNOWN_WITH_CODE にフォールバックする。
 */
export function getErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    const key = `errors:${error.code}`;
    if (i18n.exists(key)) return i18n.t(key);
    if (error.status >= 500) return i18n.t("errors:SERVER_UNAVAILABLE");
    return i18n.t("errors:UNKNOWN_WITH_CODE", { code: error.code });
  }
  // fetch 自体が失敗した場合（オフライン・DNS 失敗等）は TypeError になる。
  if (error instanceof TypeError) return i18n.t("errors:NETWORK_ERROR");
  if (error instanceof Error) return error.message;
  return i18n.t("errors:UNKNOWN");
}

type UnauthorizedHandler = () => void;
let unauthorizedHandler: UnauthorizedHandler | null = null;

/**
 * トークン失効時（401）のグローバル処理（ログアウト＋ログイン画面誘導）を登録する。
 * `AuthProvider` がマウント時に登録する。ログイン試行自体の 401（認証情報間違い）では
 * トークンが存在しないため発火しない。
 */
export function setUnauthorizedHandler(handler: UnauthorizedHandler | null) {
  unauthorizedHandler = handler;
}

function notifyIfUnauthorized(status: number) {
  if (status === 401 && getToken()) {
    unauthorizedHandler?.();
  }
}

/** レスポンスが失敗（`!res.ok`）であれば `ApiError` を投げる（`request`/`uploadFormData` で共通）。 */
export async function throwIfError(res: Response): Promise<void> {
  if (res.ok) return;
  notifyIfUnauthorized(res.status);
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

/**
 * 成功レスポンスのボディを JSON としてパースする（`request`/`uploadFormData` で共通）。
 * 204 No Content 等、ボディが無い成功レスポンスは `res.json()` が
 * "Unexpected end of JSON input" で例外を投げるため、パース前に弾く
 * （例: admin のロール変更/凍結・解除 API。処理自体は成功しているのに
 * 呼び出し側にエラーとして伝播していた不具合）。
 */
export async function parseJsonBody<T>(res: Response): Promise<T> {
  if (res.status === 204) {
    return undefined as T;
  }
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
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
  await throwIfError(res);
  return parseJsonBody<T>(res);
}

/** FormData 送信（`request()` は JSON body 前提のため通せない）用の共通エラーハンドリング。 */
async function uploadFormData<T>(path: string, formData: FormData): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method: "POST",
    headers: { ...authHeaders() },
    body: formData,
  });
  await throwIfError(res);
  return parseJsonBody<T>(res);
}

/** limit/until_id/since_id カーソルパラメータを組み立てる（7箇所の重複を共通化）。 */
export function cursorParams(params?: { limit?: number; until_id?: string; since_id?: string }): URLSearchParams {
  const q = new URLSearchParams();
  if (params?.limit) q.set("limit", String(params.limit));
  if (params?.until_id) q.set("until_id", params.until_id);
  if (params?.since_id) q.set("since_id", params.since_id);
  return q;
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
  /** 可視性（`unlisted`/`followers_only`/`direct`）。Fedi受信ポストの`to`/`cc`から判定した値。
   * `public`（デフォルト）は省略される。 */
  visibility?: string;
  /** ローカル投稿がFedi/Bskyへ実際に配送されたか。ローカル投稿以外では省略。 */
  deliverFedi?: boolean;
  deliverBsky?: boolean;
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

/** プロフィールのキーバリュー項目（#62、Mastodon 等の「プロフィールのメタデータ欄」）。 */
export interface ProfileField {
  name: string;
  value: string;
}

/** ProfileResponse（バックエンドは snake_case のまま）。 */
export interface UserProfile {
  /** DB未登録のリモートアクター（AppView直取得で未フォローのBskyユーザー等）は undefined。 */
  actor_id?: string;
  username: string;
  domain: string;
  display_name?: string;
  actor_type: string;
  ap_uri?: string;
  at_did?: string;
  bio?: string;
  avatar_url?: string;
  follow_status: "not_following" | "pending" | "accepted";
  /** 閲覧者がこのアクターをブロック中か。 */
  is_blocking: boolean;
  /** このアクターが閲覧者をブロック中か（Bsky準拠ブロックは相互完全非表示）。 */
  is_blocked_by: boolean;
  /** 閲覧者がこのアクターをミュート中か。 */
  is_muted: boolean;
  /** 最近の投稿。タイムラインと同じ NoteCard で描画する（#43）。 */
  recent_posts: Note[];
  /** ピン留め投稿（#61）。ローカルユーザーの pin/unpin 操作結果、またはリモートアクターの
   * Fedi featured collection / Bsky pinnedPost の同期結果。 */
  pinned_posts: Note[];
  /** プロフィールのキーバリュー項目（#62）。ローカル編集値、またはリモート Fedi アクターの
   * AP Actor `attachment`（`type: "PropertyValue"`）から取り込んだ値。 */
  profile_fields: ProfileField[];
  // 7.3 ブリッジ介入・魂の結合メタデータ
  bridge_real_handle?: string;
  bridge_protocol?: string; // "fedi" | "bsky"
  is_paired: boolean;
  /** 公開リスト一覧（#63）。現状ローカルユーザーのみ（リモートは将来課題）。 */
  public_lists: { id: string; name: string; member_count: number }[];
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
  visibility?: string;
  deliverFedi?: boolean;
  deliverBsky?: boolean;
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
    visibility: r.visibility,
    deliverFedi: r.deliverFedi,
    deliverBsky: r.deliverBsky,
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
  type: string; // "reaction" | "follow" | "followRequestAccepted" | "mention" | "reply"
  userId?: string;
  user?: NotificationUser;
  /** `type === "reaction"` の場合のみ。カスタム絵文字は `:shortcode:` 形式。 */
  reaction?: string;
  /**
   * `type === "reaction"` の場合は `reactionEmojis` にカスタム絵文字（`reaction` と
   * 同じキー）の画像URLが入っている場合のみ画像表示する（Unicode絵文字は入らない）。
   * `type` が `"mention"` / `"reaction"` / `"reply"` の場合は `id` があれば該当ポストへのリンクに使う。
   */
  note?: { id?: string; reactionEmojis?: Record<string, string> };
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

// ── リスト機能（#63） ──────────────────────────────────────────────────

export interface ListSummary {
  id: string;
  name: string;
  is_public: boolean;
  member_count: number;
  created_at: string;
}

export interface ListMember {
  actor_id: string;
  username: string;
  domain: string;
  display_name?: string;
  actor_type: string;
  avatar_url?: string;
  added_at: string;
}

export interface ListDetail extends ListSummary {
  members: ListMember[];
  is_owner: boolean;
}

/** アクター検索候補（リストのメンバー追加サジェスト用）。 */
export interface ActorSuggestion {
  actor_id: string;
  username: string;
  domain: string;
  display_name?: string;
  actor_type: string;
  avatar_url?: string;
  /** `api.lists.addMember` にそのまま渡せるターゲット文字列。 */
  target: string;
}

/** DMセッション一覧の相手表示情報（`handlers::dm::DmPeerResponse`）。 */
export interface DmPeer {
  id: string;
  username: string;
  domain: string;
  displayName?: string;
  actorType: string;
  avatarUrl?: string;
}

/** DMセッション（スレッド起点を同じくするdirect投稿の集合）の要約。 */
export interface DmSession {
  threadRootPostId: string;
  lastMessage: Note;
  peers: DmPeer[];
  unread: boolean;
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
      renoteId?: string,
      visibility?: "public" | "unlisted" | "followers_only" | "direct",
      recipientActorIds?: string[]
    ) {
      return normalizeNote(
        await request<RawNote>("POST", "/notes/create", {
          text,
          deliver_to_fedi: deliverToFedi,
          deliver_to_bsky: deliverToBsky,
          attachment_ids: attachmentIds.length > 0 ? attachmentIds : undefined,
          reply_to_id: replyToId,
          renote_id: renoteId,
          visibility,
          recipient_actor_ids: recipientActorIds,
        })
      );
    },
    async localTimeline(params?: { limit?: number; until_id?: string; since_id?: string; exclude_direct?: boolean }) {
      const q = cursorParams(params);
      if (params?.exclude_direct) q.set("exclude_direct", "true");
      const qs = q.toString();
      const rows = await request<RawNote[]>("GET", `/notes/local-timeline${qs ? `?${qs}` : ""}`);
      return rows.map(normalizeNote);
    },
    async homeTimeline(params?: { limit?: number; until_id?: string; since_id?: string; exclude_direct?: boolean }) {
      const q = cursorParams(params);
      if (params?.exclude_direct) q.set("exclude_direct", "true");
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
    delete(noteId: string) {
      return request<{ ok: boolean }>("DELETE", `/notes/${encodeURIComponent(noteId)}`);
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
    /** プロフィール画面の投稿一覧の追加ページ取得（無限スクロール、#64）。`actorId` は
     * `UserProfile.actor_id`（DB未登録のリモートアクターは undefined になり得る）。 */
    async posts(actorId: string, params?: { limit?: number; until_id?: string; since_id?: string; exclude_direct?: boolean }) {
      const q = cursorParams(params);
      q.set("actor_id", actorId);
      if (params?.exclude_direct) q.set("exclude_direct", "true");
      const rows = await request<RawNote[]>("GET", `/users/posts?${q.toString()}`);
      return rows.map(normalizeNote);
    },
    updateProfile(patch: {
      display_name?: string;
      bio?: string;
      avatar_media_id?: string | null;
      banner_media_id?: string | null;
      profile_fields?: ProfileField[];
    }) {
      return request<{
        username: string;
        display_name?: string;
        bio?: string;
        avatar_media_id?: number;
        banner_media_id?: number;
        profile_fields: ProfileField[];
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
      return uploadFormData<EmojiImportJob>("/admin/emojis/import", formData);
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

  blocks: {
    create(target: string) {
      return request<{ status: string }>("POST", "/blocks/create", { target });
    },
    delete(target: string) {
      return request<{ status: string }>("POST", "/blocks/delete", { target });
    },
  },

  mutes: {
    create(target: string) {
      return request<{ status: string }>("POST", "/mutes/create", { target });
    },
    delete(target: string) {
      return request<{ status: string }>("POST", "/mutes/delete", { target });
    },
  },

  actors: {
    /** DB上のアクターをユーザー名/表示名の部分一致で検索する（リストのメンバー追加サジェスト用）。 */
    search(q: string, limit = 10, signal?: AbortSignal) {
      const query = new URLSearchParams({ q, limit: String(limit) });
      return request<ActorSuggestion[]>("GET", `/actors/search?${query.toString()}`, undefined, signal);
    },
  },

  dm: {
    async sessions(params?: { limit?: number; until_id?: string; since_id?: string }) {
      const qs = cursorParams(params).toString();
      return request<DmSession[]>("GET", `/dm/sessions${qs ? `?${qs}` : ""}`);
    },
    async threadMessages(threadRootId: string, params?: { limit?: number; until_id?: string; since_id?: string }) {
      const qs = cursorParams(params).toString();
      const rows = await request<RawNote[]>("GET", `/dm/sessions/${encodeURIComponent(threadRootId)}/messages${qs ? `?${qs}` : ""}`);
      return rows.map(normalizeNote);
    },
    markRead(threadRootId: string) {
      return request<{ ok: boolean }>("POST", `/dm/sessions/${encodeURIComponent(threadRootId)}/read`);
    },
    unreadCount() {
      return request<{ count: number }>("GET", "/dm/unread-count");
    },
  },

  lists: {
    list() {
      return request<ListSummary[]>("GET", "/lists");
    },
    create(name: string, isPublic: boolean) {
      return request<ListSummary>("POST", "/lists", { name, is_public: isPublic });
    },
    get(id: string) {
      return request<ListDetail>("GET", `/lists/${encodeURIComponent(id)}`);
    },
    update(id: string, name: string, isPublic: boolean) {
      return request<ListSummary>("PATCH", `/lists/${encodeURIComponent(id)}`, { name, is_public: isPublic });
    },
    remove(id: string) {
      return request<void>("DELETE", `/lists/${encodeURIComponent(id)}`);
    },
    addMember(id: string, target: string) {
      return request<ListMember[]>("POST", `/lists/${encodeURIComponent(id)}/members`, { target });
    },
    removeMember(id: string, actorId: string) {
      return request<void>("DELETE", `/lists/${encodeURIComponent(id)}/members/${encodeURIComponent(actorId)}`);
    },
    async timeline(id: string, params?: { limit?: number; until_id?: string; since_id?: string }) {
      const qs = cursorParams(params).toString();
      const rows = await request<RawNote[]>("GET", `/lists/${encodeURIComponent(id)}/timeline${qs ? `?${qs}` : ""}`);
      return rows.map(normalizeNote);
    },
  },

  hashtags: {
    /** ホーム画面タブにピン留めされたハッシュタグ一覧。 */
    pinned() {
      return request<{ name: string }[]>("GET", "/hashtags/pinned");
    },
    pin(name: string) {
      return request<void>("POST", `/hashtags/${encodeURIComponent(name)}/pin`);
    },
    unpin(name: string) {
      return request<void>("DELETE", `/hashtags/${encodeURIComponent(name)}/pin`);
    },
    async timeline(name: string, params?: { limit?: number; until_id?: string; since_id?: string }) {
      const qs = cursorParams(params).toString();
      const rows = await request<RawNote[]>("GET", `/hashtags/${encodeURIComponent(name)}/timeline${qs ? `?${qs}` : ""}`);
      return rows.map(normalizeNote);
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
      return uploadFormData<DriveFile>("/drive/files/create", formData);
    },
  },
};

export { getToken };
