import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

export const defaultNS = "common";

/** 表示言語（設定画面「表示」＞「言語」、`users.language_preference`）。中国語のみ
 * `zh-Hant`（繁體中文）/`zh-Hans`（简体中文）のバリエーションを持つ。 */
export const displayLanguages = [
  "en",
  "ja",
  "zh-Hant",
  "zh-Hans",
  "ko",
  "es",
  "de",
  "fr",
] as const;
export type DisplayLanguage = (typeof displayLanguages)[number];

export function isDisplayLanguage(lang: string): lang is DisplayLanguage {
  return (displayLanguages as readonly string[]).includes(lang);
}

/** ポスト言語（`posts.language`、Bsky配送の`langs`にのみ反映、`docs/protocols.md`参照）。
 * 表示言語と異なり中国語はバリエーションを持たない7言語。バックエンドの
 * `seiran_common::SUPPORTED_LANGUAGES` と一致させる。 */
export const postLanguages = ["en", "ja", "zh", "ko", "es", "de", "fr"] as const;
export type PostLanguage = (typeof postLanguages)[number];

export function isPostLanguage(lang: string): lang is PostLanguage {
  return (postLanguages as readonly string[]).includes(lang);
}

/** ポスト言語選択フォームのデフォルト値（表示言語→ポスト言語への丸め）。`zh-Hant`/`zh-Hans`
 * のどちらを表示言語に選んでいても、ポスト言語のデフォルトは`zh`になる（マイケル指示）。 */
export function postLanguageBase(displayLanguage: string): PostLanguage {
  const base = displayLanguage.startsWith("zh") ? "zh" : displayLanguage;
  return isPostLanguage(base) ? base : "en";
}

const TRADITIONAL_ZH_REGIONS = ["tw", "hk", "mo"];

/**
 * ブラウザ検出（`navigator.languages`）等から得た言語コードを、`displayLanguages`の
 * 値へ正規化する。中国語は地域コードで繁體(`zh-Hant`)/简体(`zh-Hans`)を判別し、
 * 地域コードが無い/未知の地域の素の`zh`は简体字（`zh-Hans`）扱いにする。既に
 * `displayLanguages`の値そのもの（localStorageのキャッシュ値等）ならそのまま返す。
 * `isDisplayLanguage`未対応の言語コードはそのまま（i18nextのfallbackLngに委ねる）。
 */
export function normalizeDetectedLanguage(lang: string): string {
  if (isDisplayLanguage(lang)) return lang;
  const lower = lang.toLowerCase();
  if (lower.startsWith("zh")) {
    return TRADITIONAL_ZH_REGIONS.some((region) => lower.includes(`-${region}`))
      ? "zh-Hant"
      : "zh-Hans";
  }
  return lower.split("-")[0];
}

type TranslationTree = Record<string, unknown>;
type LocaleModule = { default: TranslationTree };

/**
 * 名前空間ごとの分割は、将来ユーザーが独自の言語ファイル（同形式のJSON）を
 * 作成・適用・配布できるようにする構想を見据えたもの。`i18n.addResourceBundle()`
 * で実行時にリソースを差し替え/追加できる構成にしてあるため、専用UIを追加する際も
 * ビルド済みバンドルの分解は不要。
 */
const localeModules = import.meta.glob<LocaleModule>("./locales/*/*.json", {
  eager: true,
});

export const resources = Object.fromEntries(
  displayLanguages.map((language) => [
    language,
    Object.fromEntries(
      Object.entries(localeModules)
        .filter(([path]) => path.startsWith(`./locales/${language}/`))
        .map(([path, module]) => [
          path.slice(path.lastIndexOf("/") + 1, -".json".length),
          module.default,
        ]),
    ),
  ]),
) as Record<DisplayLanguage, Record<string, TranslationTree>>;

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: "en",
    supportedLngs: displayLanguages,
    // "currentOnly": convertDetectedLanguageで既にdisplayLanguagesの値へ正規化済みの
    // コードをそのまま使う（"languageOnly"だと`zh-Hant`/`zh-Hans`が`zh`に丸められて
    // 区別できなくなってしまう）。
    load: "currentOnly",
    defaultNS,
    ns: Object.keys(resources.en),
    interpolation: { escapeValue: false },
    detection: {
      // 設定画面「表示」＞「言語」（#55）でユーザーが明示的に選択した場合は
      // localStorage に記憶し、次回以降はそれを優先する。未選択（「自動」）の
      // 場合はブラウザの言語設定（navigator）に従う。ログイン中はサーバー側の
      // 保存値（`AuthContext` 経由）がさらに優先される。
      order: ["localStorage", "navigator"],
      caches: ["localStorage"],
      convertDetectedLanguage: normalizeDetectedLanguage,
    },
    returnEmptyString: false,
  });

// `index.html` の `<html lang="ja">` は静的なプレースホルダ。実際の判定結果に同期する。
i18n.on("languageChanged", (lng) => {
  document.documentElement.lang = lng;
});

export default i18n;
