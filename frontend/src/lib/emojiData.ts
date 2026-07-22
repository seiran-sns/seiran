import dataByGroup from "unicode-emoji-json/data-by-group.json";

export interface UnicodeEmojiEntry {
  emoji: string;
  name: string;
}

export interface UnicodeEmojiGroup {
  name: string;
  emojis: UnicodeEmojiEntry[];
}

/** Unicode 公式データ（unicode-emoji-json）由来のグループ別絵文字一覧。 */
export const unicodeEmojiGroups: UnicodeEmojiGroup[] = dataByGroup.map((g) => ({
  name: g.name,
  emojis: g.emojis.map((e) => ({ emoji: e.emoji, name: e.name })),
}));

/** 検索用にグループを平坦化した全絵文字一覧。 */
export const allUnicodeEmojis: UnicodeEmojiEntry[] = unicodeEmojiGroups.flatMap((g) => g.emojis);
