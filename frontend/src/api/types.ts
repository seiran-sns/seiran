export interface User {
  id: number;
  username: string;
  email: string;
  role: string; // "user" | "moderator" | "admin"
  /** ローカル actors.id。noteUpdated ストリームイベントの reactorActorId との突き合わせに使う。 */
  actor_id: number;
  /** 左下ナビ等の自分のアイコン表示用。未設定の場合は undefined。 */
  avatar_url?: string;
  /** 表示言語設定（`ja` / `en` / `zh` / `ko` / `es` / `de` / `fr`）。`null`/`undefined` は「自動」。 */
  language_preference?: string | null;
  /** `GET /api/auth/me`のたびに再発行される新しいJWT（スライディング延命）。
   * 呼び出し側でlocalStorageへ保存し直すこと。 */
  token: string;
}

// ── 管理画面用の型（レスポンスは snake_case） ──────────────────────────────

export interface AdminUser {
  id: string;
  email: string;
  role: string;
  suspended_at: string | null;
  username: string | null;
  display_name: string | null;
  avatar_url: string | null;
  totp_enabled: boolean;
  passkey_count: number;
  /** 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（#186）。 */
  emojis?: Record<string, string>;
}

export interface AdminReport {
  id: string;
  reporter_actor_id: string;
  reporter: string;
  subject_type: "actor" | "post";
  subject_actor_id: string;
  subject: string;
  subject_post_id: string | null;
  reason_type: string;
  reason_text: string;
  destination: "local" | "remote";
  remote_host: string | null;
  status: "open" | "closed";
  forwarded_at: string | null;
  closed_at: string | null;
  created_at: string;
}
export interface ReportComment {
  id: string;
  body: string;
  author: string;
  created_at: string;
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
  site_icon_sha256: string;
  media_proxy_url: string;
  auth_bruteforce_window_minutes: string;
  auth_bruteforce_max_variants: string;
  auth_ip_block_window_minutes: string;
  auth_ip_block_threshold: string;
  auth_ip_block_duration_hours: string;
  turnstile_site_key: string;
  turnstile_secret_key_set: boolean;
  password_reset_max_active: string;
  account_creation_ip_window_minutes: string;
  account_creation_ip_max: string;
  post_rate_limit_window_minutes: string;
  post_rate_limit_max_user: string;
  post_rate_limit_max_moderator: string;
  follow_rate_limit_window_hours: string;
  follow_rate_limit_max_user: string;
  follow_rate_limit_max_moderator: string;
  list_max_count_user: string;
  list_max_count_moderator: string;
  list_member_max_user: string;
  list_member_max_moderator: string;
  search_rate_limit_window_minutes: string;
  search_rate_limit_max_user: string;
  search_rate_limit_max_moderator: string;
  oembed_allowed_domains: string;
}

export interface AuthIpBlock {
  ip_address: string;
  blocked_until: string;
  reason: string;
  created_at: string;
}

export interface CustomEmoji {
  id: string;
  shortcode: string;
  media_file_id: string;
  category: string | null;
  /** タグ（#49）。ピッカーの部分一致対象。 */
  tags: string[];
  /** ライセンス情報（#63）。1行テキスト、任意項目。 */
  license: string | null;
  created_at: string;
  /** 画像プレビュー用 URL。`listEmojis` のみ解決済み、`createEmoji` の直後レスポンスは null。 */
  url: string | null;
}

/** `GET /api/emojis`（公開、Misskey互換）の1件。バックエンドは `#[serde(rename_all = "camelCase")]`。 */
export interface PublicEmoji {
  id: string;
  aliases: string[];
  name: string;
  category: string | null;
  host: string | null;
  url: string;
  license: string | null;
  /** 画像の実寸（px）。ピッカーのアスペクト比グリッド配置に使う。 */
  width: number;
  height: number;
  /** 画像フェッチ完了までのプレースホルダ用。 */
  blurhash: string;
}

/** `GET /api/reactions/frequent` の1件（よく使う絵文字ピッカー用）。 */
export interface FrequentReaction {
  content: string;
  count: number;
  emojiUrl: string | null;
}

/** `GET /api/admin/emojis/remote` の1件。AP受信で見つけたリモートカスタム絵文字（#73）。 */
export interface RemoteEmoji {
  id: string;
  shortcode: string;
  domain: string;
  imageUrl: string;
  tags: string[];
  license: string | null;
  firstSeenAt: string;
  lastSeenAt: string;
}

export interface FediverseRelay {
  id: string;
  inbox_url: string;
  status: "pending" | "accepted" | "rejected";
  last_error: string | null;
  created_at: string;
  updated_at: string;
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

/** #65: TOTP有効化済みユーザーのログイン応答。`totp_required`の有無で`AuthResponse`と判別する。 */
export interface TotpRequiredResponse {
  totp_required: true;
  pending_token: string;
}

export type LoginResult = AuthResponse | TotpRequiredResponse;

export function isTotpRequired(res: LoginResult): res is TotpRequiredResponse {
  return "totp_required" in res;
}

export interface PasskeySummary {
  id: string;
  name: string;
  created_at: string;
  last_used_at: string | null;
}

export interface NoteAttachment {
  url: string;
  mimeType: string;
  width: number;
  height: number;
  thumbnailUrl?: string;
  durationMs?: number;
  isSensitive: boolean;
  /** GIFアニメ由来（Tenor/Klipy GIFピッカー、またはBskyのGIF直接アップロード由来）。
   * trueの場合、動画添付を自動再生・ミュート・ループ・コントロール無しで表示する。 */
  isGif: boolean;
  /** ローカルアップロードのアニメーション画像（GIF/APNG/WebPアニメ）由来かどうか。
   * リモート受信添付は常にfalse（`isGif`が別途カバーする）。Bsky embed選択（#227）で使う。 */
  isAnimatedImage: boolean;
}

/** NoteResponse（バックエンドは `#[serde(rename_all = "camelCase")]`）。 */
export interface Note {
  id: string;
  text: string;
  /** サニタイズ済みHTML（seiran Web UIでのリッチ表示用、`<blockquote>`/`<ruby>`等の構造保持）。
   * リモートFedi投稿のみ設定。無ければ`text`のプレーンテキスト描画（`RichText`）にフォールバック。 */
  contentHtml?: string;
  createdAt: string;
  user: {
    id: string | number;
    username: string;
    domain?: string;
    displayName?: string;
    actorType: string; // "local" | "fedi" | "bsky" | "remote_seiran" | ...
    avatarUrl?: string;
    /** リモート投稿者の出身インスタンス情報（Misskey API `UserLite.instance` 準拠）。
     * ローカル投稿者では省略。`themeColor` はバックエンドが宣言値/代替色/デフォルトの
     * どれかへ解決済みの最終値なので、フロントはそのまま描画すればよい。 */
    instance?: {
      name?: string;
      softwareName?: string;
      themeColor?: string;
      /** サーバーアイコン（`<link rel="icon">`優先、無ければ`/favicon.ico`）。
       * バックエンドが取得できなかった場合は省略（フロントは🌐絵文字にフォールバック）。 */
      iconUrl?: string;
    };
    /** 閲覧者から見たこのユーザーとの関係（フォロー状態・ミュート・ブロック・
     * リポストミュート）。home/local/social/globalタイムラインのみ付与される
     * （それ以外のエンドポイント経由・自分自身が対象の場合は省略）。 */
    followStatus?: "not_following" | "pending" | "accepted";
    isMuted?: boolean;
    isBlocking?: boolean;
    isBlockedBy?: boolean;
    isRepostMuted?: boolean;
  };
  attachments: NoteAttachment[];
  // 7.2 拡張メタデータ（存在する場合のみ）
  renoteId?: string;
  quoteId?: string;
  replyId?: string;
  /** `renoteId`/`quoteId`/`replyId`が無くても参照自体は存在する場合の状態
   * （`"pending"`未取り込み・`"gone"`参照先消失、#234）。対応する`*Id`があれば常に未設定。 */
  renoteStatus?: "pending" | "gone";
  quoteStatus?: "pending" | "gone";
  replyStatus?: "pending" | "gone";
  parentOriginalId?: string;
  // リアクション集計（#22）
  reactions?: ReactionSummary[];
  /** リポストの場合の元ポスト実体（#45）。この Note 自身は「リポストした」ラッパ。 */
  renote?: Note;
  /** 引用の場合の引用元ポスト実体（#116）。引用の引用は埋め込まない。 */
  quote?: Note;
  /** 認証ユーザーがこのノートをリポスト済みかどうか（未認証時は undefined）。 */
  repostedByMe?: boolean;
  /** 本文・投稿者表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（Fedi受信のみ）。
   * リアクションの画像URLは `reactions[].emojiUrl` で別途持つ。 */
  emojis?: Record<string, string>;
  /** 認証ユーザー自身の投稿がピン留め済みかどうか（#61）。自分のプロフィール表示時のみ設定。 */
  pinnedByMe?: boolean;
  /** 可視性（`unlisted`/`followers_only`/`direct`）。Fedi受信ポストの`to`/`cc`から判定した値。
   * `public`（デフォルト）は省略される。 */
  visibility?: string;
  /** ローカル投稿がFedi/Bskyへ実際に配送されたか。ローカル投稿以外では省略。 */
  deliverFedi?: boolean;
  deliverBsky?: boolean;
  /** このノートへの返信をFedi/Bskyへ配送できるか（ノート自身が実体を持つプロトコルのみ
   * true。ローカル・リモート問わず常に設定される）。リプライフォームの配送先トグルの
   * 表示・非表示に使う。 */
  replyFediAllowed: boolean;
  replyBskyAllowed: boolean;
  /** リモートBsky投稿のthreadgateにより、閲覧中ユーザーが返信できないか。 */
  replyBlocked: boolean;
  /** リモートBsky投稿のpostgateにより、閲覧中ユーザーが引用できないか。 */
  quoteBlocked: boolean;
  /** リモート投稿を元サーバー（Fedi）/ bsky.app（Bsky）上で開くための URL。ローカル投稿は省略。 */
  remoteUrl?: string;
  contentWarning?: string;
  poll?: {
    multiple: boolean;
    options: { name: string; votes: number }[];
    endTime?: string;
    closed?: string;
    votersCount?: number;
    /** ログイン中ユーザーが回答した選択肢番号。回答前・未認証時は省略。 */
    votedByMe?: number[];
  };
  /** このポストへの返信・引用・リポストの件数。 */
  replyCount: number;
  quoteCount: number;
  repostCount: number;
  /** URLカード。Bskyは`app.bsky.embed.external`由来で最大1件、Fediは本文中の複数リンクぶん
   * 複数件になりうる。無ければ空配列。 */
  linkCards: LinkCard[];
}

export interface LinkCard {
  url: string;
  title: string;
  description: string;
  thumbnailUrl?: string;
  /** oEmbed discoveryで解決され、管理者ホワイトリスト判定を通過した埋め込みプレーヤーのiframe src。 */
  embedSrc?: string;
  /** oEmbedレスポンスの`type`（"video"/"rich"等）。フロントのaspect判定には使わず、デバッグ用。 */
  embedType?: string;
}

export interface ReactionSummary {
  emoji: string;
  count: number;
  reactedByMe: boolean;
  /** カスタム絵文字（`:shortcode:`）の画像URL（ローカル送信・Fedi受信いずれも）。Unicode絵文字は undefined。 */
  emojiUrl?: string;
}

/** `GET /notes/:id/reactions/:content/actors` の1件（リアクションチップのホバーポップオーバー用）。 */
export interface ReactionActor {
  id: string;
  username: string;
  domain: string;
  displayName?: string;
  avatarUrl?: string;
}

/** `GET /notes/:id/reposts` の1件（#226 リポストタブ）。 */
export interface RepostEntry {
  /** リポストラッパー自身のポストID。`deleted`がtrueの場合は詳細画面が存在しないためリンク化しない。 */
  id: string;
  user: Note["user"];
  createdAt: string;
  /** 取り消し済み（Undo済み）リポストか。 */
  deleted: boolean;
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

/** プロフィールの「別のアカウント」1件（alsoKnownAs、seiran独自拡張）。 */
export interface AlsoKnownAsItem {
  actor_id: string;
  username: string;
  domain: string;
  display_name?: string;
  actor_type: string;
  avatar_url?: string;
  /** 相手側（fedi/ローカルのみ）も逆向きにこちらを指定していれば`true`。 */
  verified: boolean;
  last_checked_at?: string;
}

/** ProfileResponse（バックエンドは snake_case のまま）。 */
export interface UserProfile {
  /** DB未登録のリモートアクター（AppView直取得で未フォローのBskyユーザー等）は undefined。 */
  actor_id?: string;
  username: string;
  domain: string;
  display_name?: string;
  actor_type: string;
  /** サーバー名表示エリア用インスタンス情報（`Note.user.instance`と同じ形、
   * #NoteCardリモートサーバー表示）。fedi/bskyのみ、local/remote_seiranでは省略。 */
  instance?: {
    name?: string;
    softwareName?: string;
    themeColor?: string;
    iconUrl?: string;
  };
  ap_uri?: string;
  at_did?: string;
  bio?: string;
  /** 自己紹介文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（#169）。未指定/空なら絵文字化しない。 */
  emojis?: Record<string, string>;
  avatar_url?: string;
  follow_status: "not_following" | "pending" | "accepted";
  /** このアクターが閲覧者をフォロー中か（Misskey互換API `UserDetailed.isFollowed` に準拠）。 */
  is_followed: boolean;
  /** 閲覧者がこのアクターをブロック中か。 */
  is_blocking: boolean;
  /** このアクターが閲覧者をブロック中か（Bsky準拠ブロックは相互完全非表示）。 */
  is_blocked_by: boolean;
  /** 閲覧者がこのアクターをミュート中か。 */
  is_muted: boolean;
  /** 閲覧者がこのアクターをリポストミュート中か（通常投稿は表示、リポストのみ非表示にする
   * 独立フラグ）。 */
  is_repost_muted: boolean;
  /** アカウントが凍結中か。ローカルユーザーのみ判定対象（リモートアクターは常に false）。 */
  is_suspended: boolean;
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
  /** フォロー中の人数（#56）。DB未登録のリモートアクターは常に0。 */
  following_count: number;
  /** フォロワーの人数（#56）。following_count と同様、DB未登録のリモートアクターは常に0。 */
  follower_count: number;
  /** 生年月日（`YYYY-MM-DD`、Misskey互換の`birthday`）。本人が閲覧している場合は常に含まれる
   * （編集フォームの初期値用）。他人が閲覧している場合は`birthday_public=true`の時のみ。 */
  birthday?: string;
  /** `true`ならFediverseへ`vcard:bday`として公開する。本人が閲覧している場合のみ含まれる。 */
  birthday_public?: boolean;
  /** プロフィールの「別のアカウント」（alsoKnownAs、seiran独自拡張）。現状ローカルユーザーのみ
   * （`public_lists`と同様、リモートは将来課題）。 */
  also_known_as: AlsoKnownAsItem[];
}

/** フォロー中/フォロワー一覧の1件（#56、`GET /users/following` `/users/followers`）。 */
export interface FollowListItem {
  /** カーソルページネーション用（次ページ取得の `until_id` にそのまま渡す）。 */
  follow_id: string;
  actor_id: string;
  username: string;
  domain: string;
  display_name?: string;
  avatar_url?: string;
}

/** リモートFediアクターのフォロー中/フォロワー全件取得の1件（#68）。ローカルDB未登録の
 * 場合は `actor_id` が無く、`handle`/`domain` は AP actor URI から抽出した簡易表示。 */
export interface RemoteFollowSummaryItem {
  uri: string;
  actor_id?: string;
  handle: string;
  domain: string;
  display_name?: string;
  avatar_url?: string;
}

/** `GET /users/remote-follow-summary` のレスポンス（#68）。 */
export interface RemoteFollowSummaryResponse {
  items: RemoteFollowSummaryItem[];
  complete: boolean;
  /** 同期取得できず、Workerでのバックグラウンド全件取得を積んだか。 */
  pending: boolean;
  fetched_at?: string;
  /** ローカルDB把握分とリモート直接取得分をブレンドした実際のフォロー中/フォロワー数（#68）。 */
  total_count: number;
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
export interface RawNote {
  id: string | number;
  text?: string;
  contentHtml?: string;
  createdAt?: string;
  created_at?: string;
  user?: {
    id: string | number;
    username: string;
    domain?: string;
    displayName?: string;
    display_name?: string;
    actorType?: string;
    actor_type?: string;
    avatarUrl?: string;
    avatar_url?: string;
    instance?: {
      name?: string;
      softwareName?: string;
      themeColor?: string;
      iconUrl?: string;
    };
    followStatus?: "not_following" | "pending" | "accepted";
    follow_status?: "not_following" | "pending" | "accepted";
    isMuted?: boolean;
    is_muted?: boolean;
    isBlocking?: boolean;
    is_blocking?: boolean;
    isBlockedBy?: boolean;
    is_blocked_by?: boolean;
    isRepostMuted?: boolean;
    is_repost_muted?: boolean;
  };
  attachments?: NoteAttachment[];
  renoteId?: string;
  renote_id?: string;
  quoteId?: string;
  quote_id?: string;
  replyId?: string;
  reply_id?: string;
  renoteStatus?: "pending" | "gone";
  renote_status?: "pending" | "gone";
  quoteStatus?: "pending" | "gone";
  quote_status?: "pending" | "gone";
  replyStatus?: "pending" | "gone";
  reply_status?: "pending" | "gone";
  parentOriginalId?: string;
  parent_original_id?: string;
  reactions?: ReactionSummary[];
  renote?: RawNote;
  quote?: RawNote;
  repostedByMe?: boolean;
  reposted_by_me?: boolean;
  emojis?: Record<string, string>;
  pinnedByMe?: boolean;
  pinned_by_me?: boolean;
  visibility?: string;
  deliverFedi?: boolean;
  deliverBsky?: boolean;
  replyFediAllowed?: boolean;
  replyBskyAllowed?: boolean;
  replyBlocked?: boolean;
  quoteBlocked?: boolean;
  remoteUrl?: string;
  remote_url?: string;
  contentWarning?: string;
  content_warning?: string;
  poll?: Note["poll"];
  replyCount?: number;
  reply_count?: number;
  quoteCount?: number;
  quote_count?: number;
  repostCount?: number;
  repost_count?: number;
  linkCards?: LinkCard[];
  link_cards?: LinkCard[];
}

/** snake_case / camelCase 混在に耐えるノート正規化。 */
export function normalizeNote(r: RawNote): Note {
  return {
    id: String(r.id),
    text: r.text ?? "",
    contentHtml: r.contentHtml,
    createdAt: r.createdAt ?? r.created_at ?? "",
    user: {
      id: String(r.user?.id ?? ""),
      username: r.user?.username ?? "",
      domain: r.user?.domain,
      displayName: r.user?.displayName ?? r.user?.display_name,
      actorType: r.user?.actorType ?? r.user?.actor_type ?? "local",
      avatarUrl: r.user?.avatarUrl ?? r.user?.avatar_url,
      instance: r.user?.instance,
      followStatus: r.user?.followStatus ?? r.user?.follow_status,
      isMuted: r.user?.isMuted ?? r.user?.is_muted,
      isBlocking: r.user?.isBlocking ?? r.user?.is_blocking,
      isBlockedBy: r.user?.isBlockedBy ?? r.user?.is_blocked_by,
      isRepostMuted: r.user?.isRepostMuted ?? r.user?.is_repost_muted,
    },
    attachments: r.attachments ?? [],
    renoteId: r.renoteId ?? r.renote_id,
    quoteId: r.quoteId ?? r.quote_id,
    replyId: r.replyId ?? r.reply_id,
    renoteStatus: r.renoteStatus ?? r.renote_status,
    quoteStatus: r.quoteStatus ?? r.quote_status,
    replyStatus: r.replyStatus ?? r.reply_status,
    parentOriginalId: r.parentOriginalId ?? r.parent_original_id,
    reactions: r.reactions ?? [],
    renote: r.renote ? normalizeNote(r.renote) : undefined,
    quote: r.quote ? normalizeNote(r.quote) : undefined,
    repostedByMe: r.repostedByMe ?? r.reposted_by_me,
    emojis: r.emojis,
    pinnedByMe: r.pinnedByMe ?? r.pinned_by_me,
    visibility: r.visibility,
    deliverFedi: r.deliverFedi,
    deliverBsky: r.deliverBsky,
    replyFediAllowed: r.replyFediAllowed ?? false,
    replyBskyAllowed: r.replyBskyAllowed ?? false,
    replyBlocked: r.replyBlocked ?? false,
    quoteBlocked: r.quoteBlocked ?? false,
    remoteUrl: r.remoteUrl ?? r.remote_url,
    contentWarning: r.contentWarning ?? r.content_warning,
    poll: r.poll,
    replyCount: r.replyCount ?? r.reply_count ?? 0,
    quoteCount: r.quoteCount ?? r.quote_count ?? 0,
    repostCount: r.repostCount ?? r.repost_count ?? 0,
    linkCards: r.linkCards ?? r.link_cards ?? [],
  };
}

/** プロフィール「投稿」タブの投稿＋リアクション混合フィードにおける1件のリアクションイベント。 */
export interface ReactionEvent {
  id: string;
  createdAt: string;
  reaction: string;
  reactionEmojiUrl?: string;
  targetNoteId: string;
  targetUser: {
    id: string;
    username: string;
    domain?: string;
    displayName?: string;
    actorType: string;
    avatarUrl?: string;
  };
  targetUserEmojis?: Record<string, string>;
}

/** `GET /api/users/posts?includeReactions=true` の生レスポンス1件。 */
export type RawProfileFeedItem =
  | { kind: "note"; data: RawNote }
  | { kind: "reaction"; data: ReactionEvent };

/** プロフィール「投稿」タブの投稿＋リアクション混合フィード1件（正規化後）。 */
export type ProfileFeedItem =
  | { kind: "note"; note: Note }
  | { kind: "reaction"; event: ReactionEvent };

export function normalizeProfileFeedItem(raw: RawProfileFeedItem): ProfileFeedItem {
  return raw.kind === "note"
    ? { kind: "note", note: normalizeNote(raw.data) }
    : { kind: "reaction", event: raw.data };
}

/** ストリーミング（#37）で受け取った note ペイロードを Note に正規化する。 */
export function noteFromStream(body: unknown): Note {
  return normalizeNote(body as RawNote);
}

export interface FollowResponse {
  status: string;
  target_uri: string;
}

/** フォローインポート開始レスポンス（`POST /account/follow-import`）。 */
export interface FollowImportStartResponse {
  requestId: number;
  total: number;
}

/** フォローインポート進捗（`GET /account/follow-import`）。`status` が "idle" の場合は
 * 直近のインポート履歴が無い（total/succeeded/failed は全て0）。 */
export interface FollowImportStatusResponse {
  status: "idle" | "running" | "completed" | "cancelled";
  total: number;
  processed: number;
  succeeded: number;
  /** 呼び出し前から既にフォロー関係が存在していたため、新規フォローが成立しなかった件数。 */
  alreadyFollowing: number;
  failed: number;
}

/** ミュート/ブロック一覧の1件（#55、`GET /mutes` `/blocks`）。 */
export interface MutedOrBlockedActor {
  actor_id: string;
  username: string;
  domain: string;
  display_name?: string;
  avatar_url?: string;
}

/** 発行済みアプリトークンの1件（#60、`GET /account/app-tokens`）。 */
export interface AppTokenRow {
  id: string;
  client_name: string;
  created_at: string;
}

/** アプリトークン発行直後のレスポンス（`POST /account/app-tokens`）。`token`はこの
 * レスポンスでしか返らない（DBには検証用のjtiしか保存せず、再表示できないため）。 */
export interface CreateAppTokenResponse {
  id: string;
  token: string;
  client_name: string;
  created_at: string;
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
  /** アニメーション画像（GIF/APNG/WebPアニメ）由来かどうか。Bsky embed選択（#227）で
   * 「静止画」「アニメGIF」のラジオボタン項目を分けるために使う。 */
  isAnimatedImage: boolean;
}

/**
 * Bsky配送時、複数の添付候補（アンケート・静止画グループ・アニメGIF・動画・本文URL）の
 * うちどれをBsky embedにするかの明示選択（#227、バックエンド
 * `CreateNoteRequest::bsky_embed_choice`）。省略時はバックエンドが固定優先順位
 * （アンケート→静止画→アニメGIF→動画/音声→本文URL）で自動選択する。
 */
export type BskyEmbedChoice =
  | { kind: "images" }
  | { kind: "attachment"; id: string }
  | { kind: "url"; url: string }
  | { kind: "poll" };

/** アンケート作成（#228）。`api.notes.create`の`poll`引数、`CreateNoteRequest::poll`と対応。 */
export interface PollCreateInput {
  choices: string[];
  multiple?: boolean;
  /** 絶対時刻（ISO8601）。期限指定（日時）用。 */
  expiresAtIso?: string;
  /** 送信時刻からの相対秒数。期限プリセット（経過時間）用。 */
  expiresInSeconds?: number;
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
  /** `name`（表示名）中のカスタム絵文字（#186）。Misskey本家仕様に合わせコロンなし
   * shortcode がキー（`reactionEmojis` と同様、参照時は `:shortcode:` を組み立てること）。 */
  emojis?: Record<string, string>;
}

export interface NotificationItem {
  id: string;
  createdAt: string;
  // Misskey本家の notificationTypes（packages/backend/src/types.ts）準拠。
  // seiran内部では「リポスト」と呼ぶ種別もAPI上は "renote" で返す（バックエンド convert.rs で変換）。
  // "moveRefollowed" | "moveAlreadyFollowing" は Misskey本家に無いseiran独自拡張
  // （ActivityPub Move＝アカウント引っ越し受信時の再フォロー通知、`docs/protocols.md`参照）。
  type: string; // "reaction" | "follow" | "receiveFollowRequest" | "followRequestAccepted" | "mention" | "reply" | "renote" | "quote" | "moveRefollowed" | "moveAlreadyFollowing"
  userId?: string;
  user?: NotificationUser;
  /** `type === "reaction"` の場合のみ。カスタム絵文字は `:shortcode:` 形式。 */
  reaction?: string;
  /** `type === "moveRefollowed" | "moveAlreadyFollowing"` の場合のみ。引っ越し先アクター。 */
  relatedUserId?: string;
  relatedUser?: NotificationUser;
  /**
   * `type === "reaction"` の場合は `reactionEmojis` にカスタム絵文字の画像URLが
   * 入っている場合のみ画像表示する（Unicode絵文字は入らない）。キーは Misskey
   * 本家仕様に合わせコロンなし shortcode（`reaction` はコロン付き `:shortcode:`
   * 形式のため、参照時は先頭末尾の ':' を除いて引く必要がある）。
   * `type` が `"mention"` / `"reaction"` / `"reply"` / `"renote"` / `"quote"` の場合は
   * `id` があれば該当ポストへのリンクに使う。
   */
  /** `type === "renote"` の場合、この note はリポストラッパー投稿（本文なし）自体であり、
   * リポスト元の実体投稿が `renote` に埋め込まれる（`build_notes`/`embed_renotes`）。 */
  note?: { id?: string; reactionEmojis?: Record<string, string>; renote?: { id?: string } };
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
  /** 自ホストドメインが未確定の場合のみ、Hostヘッダーから判定した候補が入る。 */
  domain_candidate: string | null;
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

/** 対ユーザー操作メニューの「リストに追加/から外す」項目用（`GET /lists/membership/:actorId`）。 */
export interface ListMembership {
  id: string;
  name: string;
  is_public: boolean;
  contains: boolean;
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
  turnstileSiteKey: string;
  siteColor?: string;
  siteIconUrl?: string;
  mediaProxyUrl?: string;
  internalMediaOrigins?: string[];
}

