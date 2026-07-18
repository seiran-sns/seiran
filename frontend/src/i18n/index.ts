import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

import enCommon from "./locales/en/common.json";
import enErrors from "./locales/en/errors.json";
import enAuth from "./locales/en/auth.json";
import enSetup from "./locales/en/setup.json";
import enNav from "./locales/en/nav.json";
import enHome from "./locales/en/home.json";
import enProfile from "./locales/en/profile.json";
import enLists from "./locales/en/lists.json";
import enNotifications from "./locales/en/notifications.json";
import enSearch from "./locales/en/search.json";
import enAdmin from "./locales/en/admin.json";
import enMiauth from "./locales/en/miauth.json";
import enAccount from "./locales/en/account.json";

import jaCommon from "./locales/ja/common.json";
import jaErrors from "./locales/ja/errors.json";
import jaAuth from "./locales/ja/auth.json";
import jaSetup from "./locales/ja/setup.json";
import jaNav from "./locales/ja/nav.json";
import jaHome from "./locales/ja/home.json";
import jaProfile from "./locales/ja/profile.json";
import jaLists from "./locales/ja/lists.json";
import jaNotifications from "./locales/ja/notifications.json";
import jaSearch from "./locales/ja/search.json";
import jaAdmin from "./locales/ja/admin.json";
import jaMiauth from "./locales/ja/miauth.json";
import jaAccount from "./locales/ja/account.json";

export const defaultNS = "common";

/**
 * 名前空間ごとの分割は、将来ユーザーが独自の言語ファイル（同形式のJSON）を
 * 作成・適用・配布できるようにする構想を見据えたもの。`i18n.addResourceBundle()`
 * で実行時にリソースを差し替え/追加できる構成にしてあるため、専用UIを追加する際も
 * ビルド済みバンドルの分解は不要。
 */
export const resources = {
  en: {
    common: enCommon,
    errors: enErrors,
    auth: enAuth,
    setup: enSetup,
    nav: enNav,
    home: enHome,
    profile: enProfile,
    lists: enLists,
    notifications: enNotifications,
    search: enSearch,
    admin: enAdmin,
    miauth: enMiauth,
    account: enAccount,
  },
  ja: {
    common: jaCommon,
    errors: jaErrors,
    auth: jaAuth,
    setup: jaSetup,
    nav: jaNav,
    home: jaHome,
    profile: jaProfile,
    lists: jaLists,
    notifications: jaNotifications,
    search: jaSearch,
    admin: jaAdmin,
    miauth: jaMiauth,
    account: jaAccount,
  },
} as const;

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: "en",
    supportedLngs: ["en", "ja"],
    load: "languageOnly",
    defaultNS,
    ns: Object.keys(resources.en),
    interpolation: { escapeValue: false },
    detection: {
      // ブラウザの言語設定にのみ従う。ユーザー切り替えUIは未実装のため
      // localStorage 等へのキャッシュは行わない。
      order: ["navigator"],
      caches: [],
    },
    returnEmptyString: false,
  });

// `index.html` の `<html lang="ja">` は静的なプレースホルダ。実際の判定結果に同期する。
i18n.on("languageChanged", (lng) => {
  document.documentElement.lang = lng;
});

export default i18n;
