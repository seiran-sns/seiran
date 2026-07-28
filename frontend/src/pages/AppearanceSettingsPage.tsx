import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useAuth } from "../contexts/AuthContext";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useTheme, type ThemePreference } from "../contexts/ThemeContext";
import i18n, { supportedLanguages, type SupportedLanguage } from "../i18n";
import panel from "../components/common/Panel.module.css";
import styles from "./AppearanceSettings.module.css";

type LanguageOption = "auto" | SupportedLanguage;

const LANGUAGE_LABEL_KEYS: Record<LanguageOption, string> = {
  auto: "appearanceSettings.languageAuto",
  ja: "appearanceSettings.languageJa",
  en: "appearanceSettings.languageEn",
  zh: "appearanceSettings.languageZh",
  ko: "appearanceSettings.languageKo",
  es: "appearanceSettings.languageEs",
  de: "appearanceSettings.languageDe",
  fr: "appearanceSettings.languageFr",
};

const THEME_OPTIONS: ThemePreference[] = ["system", "light", "dark"];

const THEME_LABEL_KEYS: Record<ThemePreference, string> = {
  system: "appearanceSettings.themeSystem",
  light: "appearanceSettings.themeLight",
  dark: "appearanceSettings.themeDark",
};

/** ブラウザの言語設定から表示言語を推定する（「自動」選択時、`i18next-browser-languagedetector` の navigator 判定と同じ方針）。 */
function detectAutoLanguage(): string {
  const langs =
    navigator.languages && navigator.languages.length > 0
      ? navigator.languages
      : [navigator.language];
  for (const lang of langs) {
    const language = lang.toLowerCase().split("-")[0];
    if (supportedLanguages.includes(language as SupportedLanguage))
      return language;
  }
  return "en";
}

/** 設定画面「表示」（#55, #127）。テーマ（環境に従う/常にライト/常にダーク）と言語（自動/日本語/英語）を選択する。 */
export default function AppearanceSettingsPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const goBack = useGoBack();
  const { preference: themePreference, setPreference: setThemePreference } =
    useTheme();

  const [selected, setSelected] = useState<LanguageOption>(
    supportedLanguages.includes(user?.language_preference as SupportedLanguage)
      ? (user?.language_preference as SupportedLanguage)
      : "auto",
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  async function selectLanguage(option: LanguageOption) {
    setSelected(option);
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      await api.account.updateLanguage(option === "auto" ? null : option);
      if (option === "auto") {
        localStorage.removeItem("i18nextLng");
        await i18n.changeLanguage(detectAutoLanguage());
      } else {
        await i18n.changeLanguage(option);
      }
      setSaved(true);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>
          {t("account:appearanceSettings.title")}
        </span>
      </header>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:appearanceSettings.themeTitle")}
        </h3>
        <div className={styles.themeOptions}>
          {THEME_OPTIONS.map((option) => (
            <button
              key={option}
              type="button"
              className={`${styles.themeOption} ${themePreference === option ? styles.themeOptionActive : ""}`}
              aria-pressed={themePreference === option}
              onClick={() => setThemePreference(option)}
            >
              {t(`account:${THEME_LABEL_KEYS[option]}`)}
            </button>
          ))}
        </div>
      </div>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:appearanceSettings.languageTitle")}
        </h3>
        {error && <p className={styles.error}>{error}</p>}
        <select
          className={styles.select}
          value={selected}
          disabled={saving}
          onChange={(e) => selectLanguage(e.target.value as LanguageOption)}
        >
          {(Object.keys(LANGUAGE_LABEL_KEYS) as LanguageOption[]).map(
            (option) => (
              <option key={option} value={option}>
                {t(`account:${LANGUAGE_LABEL_KEYS[option]}`)}
              </option>
            ),
          )}
        </select>
        {saved && (
          <p className={styles.success}>
            {t("account:appearanceSettings.saved")}
          </p>
        )}
      </div>
    </>
  );

  return <AppShell center={center} />;
}
