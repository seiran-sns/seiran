import type { SupportedLanguage } from "../i18n";

interface EmojibaseEntry {
  emoji?: string;
  label: string;
  tags?: string[];
}

/** 絵文字 → 検索対象語（CLDR 正式名＋アノテーションキーワード）の索引。 */
export type EmojiAnnotationIndex = Map<string, string[]>;

const dataLoaders: Record<SupportedLanguage, () => Promise<EmojibaseEntry[]>> = {
  en: () => import("emojibase-data/en/data.json").then((m) => m.default as EmojibaseEntry[]),
  ja: () => import("emojibase-data/ja/data.json").then((m) => m.default as EmojibaseEntry[]),
  zh: () => import("emojibase-data/zh/data.json").then((m) => m.default as EmojibaseEntry[]),
  ko: () => import("emojibase-data/ko/data.json").then((m) => m.default as EmojibaseEntry[]),
  es: () => import("emojibase-data/es/data.json").then((m) => m.default as EmojibaseEntry[]),
  de: () => import("emojibase-data/de/data.json").then((m) => m.default as EmojibaseEntry[]),
  fr: () => import("emojibase-data/fr/data.json").then((m) => m.default as EmojibaseEntry[]),
};

function buildIndex(entries: EmojibaseEntry[]): EmojiAnnotationIndex {
  const index: EmojiAnnotationIndex = new Map();
  for (const entry of entries) {
    if (!entry.emoji) continue;
    index.set(entry.emoji, [entry.label, ...(entry.tags ?? [])]);
  }
  return index;
}

const indexCache = new Map<SupportedLanguage, Promise<EmojiAnnotationIndex>>();

/** 指定言語の CLDR アノテーション索引を遅延ロードする（言語ごとに一度だけフェッチ）。 */
function loadEmojiAnnotations(language: SupportedLanguage): Promise<EmojiAnnotationIndex> {
  let cached = indexCache.get(language);
  if (!cached) {
    cached = dataLoaders[language]().then(buildIndex);
    indexCache.set(language, cached);
  }
  return cached;
}

/**
 * UI 言語向けの検索索引を組み立てる。ショートコードが英語であるため英語版は常に含み、
 * UI 言語が英語以外ならその言語版も加える（例: 日本語 UI なら英語＋日本語のみ）。
 */
export async function loadEmojiAnnotationIndex(uiLanguage: SupportedLanguage): Promise<EmojiAnnotationIndex> {
  const languages = uiLanguage === "en" ? ([uiLanguage] as const) : (["en", uiLanguage] as const);
  const indexes = await Promise.all(languages.map(loadEmojiAnnotations));
  if (indexes.length === 1) return indexes[0];
  const merged: EmojiAnnotationIndex = new Map(indexes[0]);
  for (const [emoji, words] of indexes[1]) {
    merged.set(emoji, [...(merged.get(emoji) ?? []), ...words]);
  }
  return merged;
}
