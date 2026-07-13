import emojiRegex from "emoji-regex";

/**
 * 入力文字列全体が Unicode 絵文字1つ（単体・肌色/性別修飾・ZWJ結合・国旗・キーキャップ等の
 * RGI シーケンスを含む）として認識できるかを判定する。`:shortcode:` やプレーンテキストは
 * 許可しない。バックエンド（`emojis` crate による完全一致）と同じ「絵文字のみ」方針を
 * フロント側でも先取りして弾くための軽量チェックで、最終的な正当性判定は API 側が行う。
 */
export function isValidReactionEmoji(input: string): boolean {
  const trimmed = input.trim();
  if (!trimmed) return false;
  const matches = trimmed.match(emojiRegex());
  // 文字列全体が単一の絵文字トークンと完全一致する場合のみ許可する
  // （"🎉nice" のような絵文字+テキストの混在や複数絵文字の連結は拒否）。
  return matches !== null && matches.length === 1 && matches[0] === trimmed;
}
