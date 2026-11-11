import type { FrequentReaction, PublicEmoji } from "../api/client";
import { isLocalCustomEmoji, parseReactionContent } from "./customEmojis";

/** NoteCard のボタンバーに置く1クリックの絵文字。Bsky の like・AP の `Like`・Mastodon API の
 * favourite と相互に変換される「いいね」相当のリアクションなので、使用頻度に関わらず固定で置く。
 * クイックリアクションのポップアップ（よく使う絵文字）には並べない。 */
export const QUICK_REACTION_HEART = "❤️";

export interface QuickReaction {
  /** リアクションとして送信する値（Unicode絵文字文字列 or `:shortcode:`）。 */
  content: string;
  /** カスタム絵文字の画像URL（Unicode絵文字なら undefined）。 */
  imageUrl?: string;
  /** カスタム絵文字の画像の実寸（px）。画像の読み込み前から横幅を確保するのに使う。 */
  width?: number;
  height?: number;
}

/** VS16（U+FE0F）の有無だけが違う Unicode 絵文字を同じものとして扱うための正規化。 */
function stripVariationSelector(s: string): string {
  return s.replace(/️/g, "");
}

/**
 * 2つのリアクション値が同じ絵文字を指すか。カスタム絵文字は送信時の `:shortcode:` と保存値の
 * `:shortcode@.:`（ローカル）で表記が異なるため、shortcode とローカルかどうかで比べる。
 * Unicode 絵文字は VS16 の有無を無視して比べる（`❤` と `❤️` が別の絵文字扱いにならないように）。
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
 * クイックリアクションのポップアップに並べる絵文字を、よく使う順（`GET /api/reactions/frequent`
 * の順）のまま選ぶ。❤️はボタンバーに常にあるので、上位でも並べない。カスタム絵文字は送信できる
 * （このサーバーに登録済みの）ものだけを候補にする（未登録のものは `create_reaction` が
 * `UNKNOWN_EMOJI` で拒否する）。横長のものも除かず、何個並ぶかはポップアップの横幅で決まる。
 */
export function selectQuickReactions(
  frequent: FrequentReaction[],
  customEmojis: PublicEmoji[]
): QuickReaction[] {
  const customByName = new Map(customEmojis.map((e) => [e.name, e]));
  const result: QuickReaction[] = [];
  for (const f of frequent) {
    if (isSameReaction(f.content, QUICK_REACTION_HEART)) continue;
    if (result.some((q) => isSameReaction(q.content, f.content))) continue;
    const parsed = parseReactionContent(f.content);
    if (!parsed) {
      result.push({ content: f.content });
      continue;
    }
    if (!isLocalCustomEmoji(parsed)) continue;
    const emoji = customByName.get(parsed.shortcode);
    if (!emoji) continue;
    result.push({
      content: `:${emoji.name}:`,
      imageUrl: emoji.url,
      width: emoji.width,
      height: emoji.height,
    });
  }
  return result;
}

/**
 * 取り直した候補 `next` を、表示中の `prev` の並びをなるべく保って並べ直す。引き続き候補に残る
 * 絵文字は同じ位置に留め、外れた絵文字の位置にだけ新しく入った絵文字を頻度順に入れる。
 * リアクションした直後の取り直しでポップアップの並びが入れ替わると、続けて押そうとした位置に
 * 別の絵文字が来てしまうため（順位どおりに並べ直すのはページの読み込み直しと時間帯の境目）。
 */
export function mergeQuickReactionsStable(
  prev: QuickReaction[],
  next: QuickReaction[]
): QuickReaction[] {
  const result: (QuickReaction | null)[] = prev.map(
    (p) => next.find((n) => n.content === p.content) ?? null
  );
  const incoming = next.filter((n) => !prev.some((p) => p.content === n.content));
  for (let i = 0; i < result.length && incoming.length > 0; i++) {
    if (result[i] === null) result[i] = incoming.shift()!;
  }
  return [...result.filter((q): q is QuickReaction => q !== null), ...incoming];
}
