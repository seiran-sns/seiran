import { MENTION_SOURCE } from "./richTextPatterns";

/**
 * プロフィールのbio/profile_fields値を`RichHtml`で表示するためのHTML化。
 *
 * リモートFedi/remote_seiranアクターの値はバックエンドで既にallowlistサニタイズ済みのHTML
 * （`sanitize_html_allowlist`、投稿本文と同じルール）。ローカル/Bskyアクターの値はプレーン
 * テキストのままDB保存されているため、表示直前にエスケープ＋裸URL/メンション記法のリンク化
 * ＋改行の`<br>`化を行い、同じ`RichHtml`（`dangerouslySetInnerHTML`を使わない多層防御パーサー）
 * に通す。裸URL・メンション記法（`@user@host`/`@handle.bsky.social`）はいずれも`<a href="...">`
 * として埋め込み、`href`をキーに`ProfilePage`の`resolvedLinks`（#リンク解決）と突き合わせる
 * ことで、解決済みならホバースイッチ・右クリックメニュー・顔アイコン付きのサイト内リンクへ
 * `RichHtml`側で差し替わる（`RichText`の`@mention`即時リンク化とは異なり、実在確認済みの
 * リンクだけをリッチ化する設計）。
 */

const URL_SOURCE = String.raw`https?://[^\s<>()[\]]+`;
const LINKIFY_SOURCE = `(?<url>${URL_SOURCE})|${MENTION_SOURCE}`;

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/** バックエンドがbio/profile_fields値をHTMLとして保存するアクター種別か。 */
export function isRemoteHtmlActor(actorType: string): boolean {
  return actorType === "fedi" || actorType === "remote_seiran";
}

/**
 * プレーンテキストをエスケープした上で、裸URL・メンション記法（`@user@host`/
 * `@handle.bsky.social`）をリンク化し改行を`<br>`に変換する。
 */
export function escapeAndLinkify(text: string): string {
  const re = new RegExp(LINKIFY_SOURCE, "gu");
  let result = "";
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    result += escapeHtml(text.slice(lastIndex, match.index));
    const g = match.groups!;
    if (g.url !== undefined) {
      const url = escapeHtml(g.url);
      result += `<a href="${url}">${url}</a>`;
    } else if (g.mention !== undefined) {
      const mention = escapeHtml(`@${g.mention}`);
      result += `<a href="${mention}">${mention}</a>`;
    }
    lastIndex = match.index + match[0].length;
  }
  result += escapeHtml(text.slice(lastIndex));
  return result.replace(/\n/g, "<br>");
}

/** bio/profile_fields値を`RichHtml`に渡せるHTML文字列にする（分岐の窓口）。 */
export function toProfileHtml(raw: string, actorType: string): string {
  return isRemoteHtmlActor(actorType) ? raw : escapeAndLinkify(raw);
}
