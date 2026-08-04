import type { ReactNode } from "react";
import { parse as parseTwemoji } from "@twemoji/parser";

/**
 * Unicode絵文字はOS/ブラウザごとにグリフが異なり見た目が揃わないため、jdecked/twemoji（旧
 * twitter/twemoji）のSVGをセルフホストして統一表示する。アセットは `scripts/copy-twemoji-assets.mjs`
 * が `node_modules/@twemoji/svg` から `public/twemoji/` へビルド時にコピーする（git管理外）。
 */
export interface TwemojiMatch {
  /** マッチしたUnicode絵文字の文字列（サロゲートペア込み）。 */
  text: string;
  /** セルフホストSVGのURL。 */
  url: string;
  /** `text` 内での [開始, 終了) インデックス。 */
  indices: [number, number];
}

/** `text` 中のUnicode絵文字を検出し、セルフホストSVGのURLとともに返す。絵文字が無ければ空配列。 */
export function findTwemojiMatches(text: string): TwemojiMatch[] {
  return parseTwemoji(text, {
    assetType: "svg",
    buildUrl: (codepoints) => `/twemoji/${codepoints}.svg`,
  });
}

/** 単一の絵文字文字列（"🎉" 等）をセルフホストSVGのURLに変換する。非絵文字文字列を渡した場合はundefined。 */
export function singleEmojiToTwemojiUrl(emoji: string): string | undefined {
  const matches = findTwemojiMatches(emoji);
  return matches.length === 1 && matches[0].text === emoji ? matches[0].url : undefined;
}

/** テキスト内のUnicode絵文字を、セルフホストのtwemoji SVG（`<img>`）に置換したノード配列を返す。
 * 絵文字が無ければ元の文字列を1要素の配列で返す。`EmojiText`/`TwemojiText` から共用する。 */
export function renderTextWithTwemoji(text: string, keyPrefix: string, imgClassName?: string): ReactNode[] {
  const matches = findTwemojiMatches(text);
  if (matches.length === 0) return [text];
  const nodes: ReactNode[] = [];
  let cursor = 0;
  matches.forEach((m, i) => {
    const [start, end] = m.indices;
    if (start > cursor) nodes.push(text.slice(cursor, start));
    nodes.push(
      <img
        key={`${keyPrefix}-tw${i}`}
        className={imgClassName}
        src={m.url}
        alt={m.text}
        draggable={false}
        loading="lazy"
      />
    );
    cursor = end;
  });
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}
