import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

export const defaultNS = "common";
export const supportedLanguages = [
  "en",
  "ja",
  "zh",
  "ko",
  "es",
  "de",
  "fr",
] as const;
export type SupportedLanguage = (typeof supportedLanguages)[number];

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
  supportedLanguages.map((language) => [
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
) as Record<SupportedLanguage, Record<string, TranslationTree>>;

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: "en",
    supportedLngs: supportedLanguages,
    load: "languageOnly",
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
    },
    returnEmptyString: false,
  });

// `index.html` の `<html lang="ja">` は静的なプレースホルダ。実際の判定結果に同期する。
i18n.on("languageChanged", (lng) => {
  document.documentElement.lang = lng;
});

export default i18n;
