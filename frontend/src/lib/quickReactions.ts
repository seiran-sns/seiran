import type { FrequentReaction, PublicEmoji } from "../api/client";
import { emojiAspectSpan } from "./emojiAspect";
import { isLocalCustomEmoji, parseReactionContent } from "./customEmojis";

/** クイックリアクションボタンの先頭に常に置く絵文字。Bsky の like・AP の `Like`・Mastodon API の
 * favourite と相互に変換される「いいね」相当のリアクションなので、使用頻度に関わらず固定で並べる。 */
export const QUICK_REACTION_HEART = "❤️";

/** ❤️ の後ろに並べる「よく使う絵文字」の最大数。 */
export const QUICK_REACTION_FREQUENT_LIMIT = 3;

export interface QuickReaction {
  /** リアクションとして送信する値（Unicode絵文字文字列 or `:shortcode:`）。 */
  content: string;
  /** カスタム絵文字の画像URL（Unicode絵文字なら undefined）。 */
  imageUrl?: string;
}

/** VS16（U+FE0F）の有無だけが違う Unicode 絵文字を同じものとして扱うための正規化。 */
function stripVariationSelector(s: string): string {
  return s.replace(/️/g, "");
}

/**
 * 2つのリアクション値が同じ絵文字を指すか。カスタム絵文字は送信時の `:shortcode:` と保存値の
 * `:shortcode@.:`（ローカル）で表記が異なるため、shortcode とローカルかどうかで比べる。
 * Unicode 絵文字は VS16 の有無を無視して比べる（`❤` と `❤️` が別ボタン扱いにならないように）。
 */
export function isSameReaction(a: string, b: string): boolean {
  const pa = parseReactionContent(a);
  const pb = parseReactionContent(b);
  if (pa || pb) {
    if (!pa || !pb || pa.shortcode !== pb.shortcode) return false;
    const localA = isLocalCustomEmoji(pa);
    const localB = isLocalCustomEmoji(pb);
    return localA && localB ? true : !localA && !localB && pa.host === pb.host;
  }
  return stripVariationSelector(a) === stripVariationSelector(b);
}

/**
 * クイックリアクションボタンに並べる絵文字を決める: 先頭に ❤️、続いて自分がよく使う絵文字のうち
 * 絵文字ピッカーで1マス幅（`emojiAspectSpan` が1）に収まるものを頻度順に最大3つ。
 * カスタム絵文字は送信できる（このサーバーに登録済みの）ものだけを候補にする（未登録のものは
 * `create_reaction` が `UNKNOWN_EMOJI` で拒否する。寸法も登録済み一覧からしか分からない）。
 */
export function selectQuickReactions(
  frequent: FrequentReaction[],
  customEmojis: PublicEmoji[]
): QuickReaction[] {
  const customByName = new Map(customEmojis.map((e) => [e.name, e]));
  const result: QuickReaction[] = [{ content: QUICK_REACTION_HEART }];
  for (const f of frequent) {
    if (result.length > QUICK_REACTION_FREQUENT_LIMIT) break;
    if (result.some((q) => isSameReaction(q.content, f.content))) continue;
    const parsed = parseReactionContent(f.content);
    if (!parsed) {
      result.push({ content: f.content });
      continue;
    }
    if (!isLocalCustomEmoji(parsed)) continue;
    const emoji = customByName.get(parsed.shortcode);
    if (!emoji || emojiAspectSpan(emoji.width, emoji.height) !== 1) continue;
    result.push({ content: `:${emoji.name}:`, imageUrl: emoji.url });
  }
  return result;
}
