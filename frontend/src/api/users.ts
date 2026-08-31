import { cursorParams, request } from "./core";
import type { ActorSuggestion, AlsoKnownAsItem, FollowListItem, MutedOrBlockedActor, PasskeySummary, ProfileField, ProfileFeedItem, RawNote, RawProfileFeedItem, RemoteFollowSummaryResponse, UserProfile } from "./types";
import { normalizeNote, normalizeProfileFeedItem } from "./types";
import { credentialJson, registrationOptions } from "./webauthn";
import type { RegistrationOptionsJson, WebAuthnEnvelope } from "./webauthn";

export const users = {
  async profile(q: string) {
    const raw = await request<
      Omit<UserProfile, "recent_posts" | "pinned_posts"> & {
        recent_posts?: RawNote[];
        pinned_posts?: RawNote[];
      }
    >("GET", `/users/profile?q=${encodeURIComponent(q)}`);
    // recent_posts / pinned_posts はタイムラインと同じ NoteCard で描画するため Note に正規化（#43, #61）。
    return {
      ...raw,
      recent_posts: (raw.recent_posts ?? []).map(normalizeNote),
      pinned_posts: (raw.pinned_posts ?? []).map(normalizeNote),
    } as UserProfile;
  },
  /** プロフィール画面「投稿」タブの一覧取得（初回・無限スクロール追加ページとも、#64）。
   * `actorId` は `UserProfile.actor_id`（DB未登録のリモートアクターは undefined になり得る）。
   * このユーザーが行った絵文字リアクションイベントも投稿と時系列混合で返す
   * （`includeReactions=true`、ユーザープロフィールページ「投稿」タブの絵文字リアクション
   * イベント混合表示）。 */
  async posts(
    actorId: string,
    params?: {
      limit?: number;
      until_id?: string;
      since_id?: string;
      exclude_direct?: boolean;
    },
  ): Promise<ProfileFeedItem[]> {
    const q = cursorParams(params);
    q.set("actor_id", actorId);
    if (params?.exclude_direct) q.set("exclude_direct", "true");
    q.set("include_reactions", "true");
    const rows = await request<RawProfileFeedItem[]>(
      "GET",
      `/users/posts?${q.toString()}`,
    );
    return rows.map(normalizeProfileFeedItem);
  },
  /** プロフィール画面「フォロー中」タブの一覧取得（無限スクロール、#56）。 */
  following(
    actorId: string,
    params?: { limit?: number; until_id?: string; since_id?: string },
  ) {
    const q = cursorParams(params);
    q.set("actor_id", actorId);
    return request<FollowListItem[]>(
      "GET",
      `/users/following?${q.toString()}`,
    );
  },
  /** プロフィール画面「フォロワー」タブの一覧取得（無限スクロール、#56）。 */
  followers(
    actorId: string,
    params?: { limit?: number; until_id?: string; since_id?: string },
  ) {
    const q = cursorParams(params);
    q.set("actor_id", actorId);
    return request<FollowListItem[]>(
      "GET",
      `/users/followers?${q.toString()}`,
    );
  },
  /** リモートFediアクターのフォロー中/フォロワーをAP経由で全件取得する（#68）。
   * ローカルDBが把握している範囲を超えた「相手サーバー上の実際の全件」を返す。 */
  remoteFollowSummary(actorId: string, direction: "following" | "followers") {
    const q = new URLSearchParams({ actor_id: actorId, direction });
    return request<RemoteFollowSummaryResponse>(
      "GET",
      `/users/remote-follow-summary?${q.toString()}`,
    );
  },
  updateProfile(patch: {
    display_name?: string;
    bio?: string;
    avatar_media_id?: string | null;
    banner_media_id?: string | null;
    profile_fields?: ProfileField[];
    /** `YYYY-MM-DD`、`null`で削除。 */
    birthday?: string | null;
    /** `true`ならFediverseへ`vcard:bday`として公開する（デフォルト`false`）。 */
    birthday_public?: boolean;
  }) {
    return request<{
      username: string;
      display_name?: string;
      bio?: string;
      avatar_media_id?: number;
      banner_media_id?: number;
      profile_fields: ProfileField[];
      birthday?: string;
      birthday_public: boolean;
    }>("PATCH", "/users/profile", patch);
  },
};

/** プロフィールの「別のアカウント」（alsoKnownAs、seiran独自拡張）。 */
export const alsoKnownAs = {
  add(target: string) {
    return request<AlsoKnownAsItem[]>("POST", "/users/also-known-as", { target });
  },
  remove(actorId: string) {
    return request<AlsoKnownAsItem[]>(
      "DELETE",
      `/users/also-known-as/${encodeURIComponent(actorId)}`,
    );
  },
};

export const blocks = {
  create(target: string) {
    return request<{ status: string }>("POST", "/blocks/create", { target });
  },
  delete(target: string) {
    return request<{ status: string }>("POST", "/blocks/delete", { target });
  },
  /** 設定画面のブロック一覧（#55）。 */
  list() {
    return request<MutedOrBlockedActor[]>("GET", "/blocks");
  },
};

export const mutes = {
  create(target: string) {
    return request<{ status: string }>("POST", "/mutes/create", { target });
  },
  delete(target: string) {
    return request<{ status: string }>("POST", "/mutes/delete", { target });
  },
  /** 設定画面のミュート一覧（#55）。 */
  list() {
    return request<MutedOrBlockedActor[]>("GET", "/mutes");
  },
};

export const actors = {
  /** DB上のアクターを表示名・ハンドルの部分一致で検索する（リスト・DM用）。 */
  search(q: string, limit = 10, signal?: AbortSignal) {
    const query = new URLSearchParams({ q, limit: String(limit) });
    return request<ActorSuggestion[]>(
      "GET",
      `/actors/search?${query.toString()}`,
      undefined,
      signal,
    );
  },
  /** ハンドルの前方一致で検索する（投稿欄の@サジェスト用、qに先頭@は含めない）。 */
  suggest(q: string, limit = 10, signal?: AbortSignal) {
    const query = new URLSearchParams({ q, limit: String(limit) });
    return request<ActorSuggestion[]>(
      "GET",
      `/actors/suggest?${query.toString()}`,
      undefined,
      signal,
    );
  },
};

export const account = {
  withdraw(confirmHandle: string) {
    return request<void>("POST", "/account/withdraw", {
      confirm_handle: confirmHandle,
    });
  },
  /** 設定画面のアカウント設定からパスワードを変更する（#55、要現パスワード確認）。 */
  changePassword(currentPassword: string, newPassword: string) {
    return request<void>("POST", "/account/change-password", {
      current_password: currentPassword,
      new_password: newPassword,
    });
  },
  /** 発行済みの全JWT（このリクエスト自身も含む）を一括失効させる。成功後は
   * このセッションも無効になるため、呼び出し側でログアウト処理を行うこと。 */
  revokeAllSessions() {
    return request<void>("POST", "/account/revoke-all-sessions");
  },
  /** 設定画面「表示」から表示言語を変更する（#55、`null` で自動に戻す）。 */
  updateLanguage(language: string | null) {
    return request<void>("POST", "/account/language", { language });
  },
  /** 設定画面「プライバシー」の現在値を取得する。Bsky Discoverフィード等からの除外要求。 */
  getContentVisibility() {
    return request<{ hide_from_algorithmic_recommendations: boolean }>(
      "GET",
      "/account/content-visibility",
    );
  },
  /** 設定画面「プライバシー」から、Bsky Discoverフィード等のアルゴリズムレコメンドから
   * 除外するよう要求するかどうかを更新する。 */
  updateContentVisibility(hideFromAlgorithmicRecommendations: boolean) {
    return request<{ hide_from_algorithmic_recommendations: boolean }>(
      "POST",
      "/account/content-visibility",
      { hide_from_algorithmic_recommendations: hideFromAlgorithmicRecommendations },
    );
  },
  /** 設定画面のアカウント設定からメールアドレス変更をリクエストする（#59、新アドレス宛に確認メール送信）。 */
  requestEmailChange(newEmail: string) {
    return request<void>("POST", "/account/email/request-change", {
      new_email: newEmail,
    });
  },
  /** 確認メールのリンク（`/verify-email-change?token=...`）を踏んだ際にトークンを確定する（#59）。 */
  confirmEmailChange(token: string) {
    return request<void>("POST", "/account/email/confirm-change", { token });
  },
  totp: {
    /** 設定画面「二段階認証」の現在の状態（#65）。 */
    status() {
      return request<{ enabled: boolean }>("GET", "/account/totp/status");
    },
    /** シークレットを新規生成する（未確定、`enable`で確認コード検証するまで有効にならない）。 */
    setup() {
      return request<{ secret: string; otpauth_url: string }>(
        "POST",
        "/account/totp/setup",
      );
    },
    /** 確認コードを検証して有効化する。成功時のみ表示するリカバリーコードを10件返す。 */
    enable(code: string) {
      return request<{ recovery_codes: string[] }>(
        "POST",
        "/account/totp/enable",
        { code },
      );
    },
    /** 現在のパスワード確認の上で無効化する。 */
    disable(currentPassword: string) {
      return request<void>("POST", "/account/totp/disable", {
        current_password: currentPassword,
      });
    },
  },
  passkeys: {
    list() {
      return request<PasskeySummary[]>("GET", "/account/passkeys");
    },
    async register(name: string) {
      const start = await request<WebAuthnEnvelope<RegistrationOptionsJson>>(
        "POST",
        "/account/passkeys/registration/start",
        { name },
      );
      const credential = (await navigator.credentials.create({
        publicKey: registrationOptions(start.public_key.publicKey),
      })) as PublicKeyCredential | null;
      if (!credential) throw new Error("Passkey registration was cancelled");
      return request<PasskeySummary>(
        "POST",
        "/account/passkeys/registration/finish",
        {
          token: start.token,
          credential: credentialJson(credential),
        },
      );
    },
    delete(id: string) {
      return request<void>(
        "DELETE",
        `/account/passkeys/${encodeURIComponent(id)}`,
      );
    },
  },
};
