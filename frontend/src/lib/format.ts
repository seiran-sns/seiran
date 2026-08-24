import { Note, UserProfile } from "../api/client";
import i18n from "../i18n";

/** ISO 文字列を現在の表示言語の短い日時表記に変換する。 */
export function formatDate(iso: string): string {
  return new Date(iso).toLocaleString(i18n.language, {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 返信数・引用数・リポスト数・リアクション数などの件数表示。1000未満はそのまま、
 * 1000以上は K（小数点以下1桁まで）、100万以降は M 表示にし、常に3桁以下に収める。 */
export function formatCount(n: number): string {
  if (n < 1000) return String(n);
  const isMillion = n >= 1_000_000;
  const divisor = isMillion ? 1_000_000 : 1000;
  const suffix = isMillion ? "M" : "K";
  const value = n / divisor;
  const intPart = Math.floor(value);
  if (intPart < 10) {
    const truncated = Math.floor(value * 10) / 10;
    return `${truncated.toFixed(1)}${suffix}`;
  }
  return `${intPart}${suffix}`;
}

/** ノートの表示名（display_name 優先、なければ username）。 */
export function displayName(note: Note): string {
  return note.user.displayName || note.user.username;
}

/** ノート投稿者の acct 文字列（`@user` または `@user@domain`）。ローカルユーザーは domain を省略。 */
export function acct(note: Note): string {
  return note.user.domain && note.user.domain !== window.location.hostname
    ? `@${note.user.username}@${note.user.domain}`
    : `@${note.user.username}`;
}

/** プロフィール遷移用のクエリ文字列を組み立てる（ローカルは domain を省略）。 */
export function profileQuery(username: string, domain?: string): string {
  return domain && domain !== window.location.hostname
    ? `${username}@${domain}`
    : username;
}

/** プロフィールの permalink パス（Misskey 互換の `/@handle` 形式・#36）。 */
export function profilePath(username: string, domain?: string): string {
  return `/@${profileQuery(username, domain)}`;
}

/** リモートユーザーを元サーバー（Fedi）/ bsky.app（Bsky）上で開くための URL。ローカルなら null。 */
export function remoteProfileUrl(profile: UserProfile): string | null {
  if (profile.actor_type === "local") return null;
  if (profile.ap_uri) return profile.ap_uri;
  if (profile.at_did) return `https://bsky.app/profile/${profile.at_did}`;
  return null;
}

/** themeColor未宣言・Bsky共通の汎用デフォルト（薄いグレー）。バックエンドの
 * `remote_instance_info_resolve::DEFAULT_THEME_COLOR` と揃えている。 */
export const REMOTE_SERVER_BADGE_FALLBACK_COLOR = "#e4e4e7";

/** リモートサーバー表示（#NoteCardリモートサーバー表示）の元データ。NoteCardの
 * `note.user`、プロフィール画面の `profile` のどちらからも同じ形に正規化して渡す。 */
export interface RemoteServerSubject {
  actorType: string;
  domain?: string;
  instance?: { name?: string; themeColor?: string; iconUrl?: string };
}

/** リモートサーバー表示のバッジ情報（アイコン種別・ラベル・背景色）を計算する。
 * 実際のアイコン描画（Blueskyロゴ/instanceアイコン画像）は呼び出し側に委ねる
 * （NoteCardは横並びの小バッジ、プロフィール画面はIDの下のブロックと見た目が異なるため）。
 * Bskyは固定表示、Fediはバックエンドが解決したインスタンス情報（`instance`）を使う。
 * ローカル・seiran間連合（remote_seiran）では表示しない。 */
export function remoteServerBadgeInfo(
  subject: RemoteServerSubject,
): { useBlueskyLogo: boolean; iconUrl?: string; label: string; bg: string } | null {
  if (subject.actorType === "bsky") {
    return { useBlueskyLogo: true, label: "Bluesky", bg: REMOTE_SERVER_BADGE_FALLBACK_COLOR };
  }
  if (subject.actorType === "fedi") {
    // バックエンドはfaviconの実在確認まで済ませてからiconUrlを返すため、ここでは
    // 有無だけで判定すればよい。取得できなかった場合はアイコン無し（🌐等への
    // フォールバックはしない）。
    return {
      useBlueskyLogo: false,
      iconUrl: subject.instance?.iconUrl,
      label: subject.instance?.name || subject.domain || "",
      bg: subject.instance?.themeColor || REMOTE_SERVER_BADGE_FALLBACK_COLOR,
    };
  }
  return null;
}

/** アクター種別に対応するプロトコルバッジ（絵文字 + ラベル）。 */
export function protocolBadge(actorType: string): { icon: string; label: string } | null {
  switch (actorType) {
    case "bsky":
      return { icon: "🦋", label: "Bluesky" };
    case "fedi":
      return { icon: "🌐", label: "Fediverse" };
    case "remote_seiran":
      return { icon: "🀄", label: "seiran" };
    case "local":
      return null; // ローカルはバッジ不要
    default:
      return null;
  }
}

/** ローカル投稿の配送先バッジ。`protocol`はアイコン文字列ではなく配送先そのものを表す
 * 判別子で、描画側（NoteCard）がこれを見てアイコン（絵文字/SVGロゴ等）を選ぶ。
 * ローカル投稿以外は空。 */
export function deliveryBadges(
  note: Note,
): { protocol: "fedi" | "bsky"; label: string }[] {
  if (note.user.actorType !== "local") return [];
  const badges: { protocol: "fedi" | "bsky"; label: string }[] = [];
  if (note.deliverFedi) badges.push({ protocol: "fedi", label: i18n.t("home:badges.deliveredFedi") });
  if (note.deliverBsky) badges.push({ protocol: "bsky", label: i18n.t("home:badges.deliveredBsky") });
  return badges;
}

/** ポストの可視性バッジ（🔒️プライベート/🤫ひかえめ）。public/directはアイコン無し。
 * ローカル投稿・Fedi受信投稿の両方に対応（ローカルは投稿作成時の選択、Fedi受信はto/ccから判定）。 */
export function visibilityBadge(note: Note): { icon: string; label: string } | null {
  switch (note.visibility) {
    case "followers_only":
      return { icon: "🔒️", label: i18n.t("home:badges.visibilityPrivate") };
    case "unlisted":
      return { icon: "🤫", label: i18n.t("home:badges.visibilityUnlisted") };
    default:
      return null;
  }
}

const segmenter = new Intl.Segmenter();

export function countGraphemes(text: string): number {
  return [...segmenter.segment(text)].length;
}

export function countUtf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Bsky 配信時は 300grapheme/3,000B、それ以外は 3,000grapheme/10,000B の残数を返す。 */
export function calcRemaining(text: string, deliverBsky: boolean): number {
  const maxBytes = deliverBsky ? 3_000 : 10_000;
  const maxGraphemes = deliverBsky ? 300 : 3_000;
  const graphemes = countGraphemes(text);
  const bytes = countUtf8Bytes(text);
  return Math.min(maxGraphemes - graphemes, Math.floor((maxBytes - bytes) / 3));
}

const BODY_URL_RE = /https?:\/\/[^\s<>()[\]]+/g;

/**
 * 本文中の生URLを出現順に検出し、重複を除いて返す（上限5件）。バックエンドの
 * `seiran_common::net::extract_body_urls` と同じルール・上限で、Bsky embed選択（#227）の
 * 投稿フォーム側候補一覧の算出に使う。
 */
export function extractBodyUrls(text: string): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const match of text.matchAll(BODY_URL_RE)) {
    const url = match[0];
    if (seen.has(url)) continue;
    seen.add(url);
    result.push(url);
    if (result.length >= 5) break;
  }
  return result;
}
