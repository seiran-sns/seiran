import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useAuth } from "../contexts/AuthContext";
import { useTheme, type ThemePreference } from "../contexts/ThemeContext";
import i18n, {
  normalizeDetectedLanguage,
  isDisplayLanguage,
  type DisplayLanguage,
} from "../i18n";
import { setDataSaver, useDataSaver } from "../stores/dataSaverStore";
import styles from "./AppearanceSettings.module.css";

type LanguageOption = "auto" | DisplayLanguage;

const LANGUAGE_LABEL_KEYS: Record<LanguageOption, string> = {
  auto: "appearanceSettings.languageAuto",
  ja: "appearanceSettings.languageJa",
  en: "appearanceSettings.languageEn",
  "zh-Hant": "appearanceSettings.languageZhHant",
  "zh-Hans": "appearanceSettings.languageZhHans",
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
    const normalized = normalizeDetectedLanguage(lang);
    if (isDisplayLanguage(normalized)) return normalized;
  }
  return "en";
}

/** 設定画面「表示」（#55, #127）。アカウントに保存する言語・ワンタッチリアクションバーの表示と、
 * 「この端末だけの設定」としてこの端末の `localStorage` に保存するテーマ・ギガ節約モードを選択する。 */
export default function AppearanceSettingsPage() {
  const { t } = useTranslation();
  const { user, updateUser } = useAuth();
  const { preference: themePreference, setPreference: setThemePreference } =
    useTheme();
  const dataSaver = useDataSaver();

  const [selected, setSelected] = useState<LanguageOption>(
    isDisplayLanguage(user?.language_preference ?? "")
      ? (user?.language_preference as DisplayLanguage)
      : "auto",
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [quickReactionsSaving, setQuickReactionsSaving] = useState(false);
  const [quickReactionsError, setQuickReactionsError] = useState("");
  // この項目を返さない版のサーバーでは、従来どおり表示されている。
  const showQuickReactions = user?.show_quick_reactions ?? true;

  async function toggleQuickReactions(show: boolean) {
    setQuickReactionsSaving(true);
    setQuickReactionsError("");
    try {
      await api.account.updateQuickReactions(show);
      updateUser({ show_quick_reactions: show });
    } catch (err) {
      setQuickReactionsError(getErrorMessage(err));
    } finally {
      setQuickReactionsSaving(false);
    }
  }

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

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:appearanceSettings.quickReactionsTitle")}
        </h3>
        {quickReactionsError && <p className={styles.error}>{quickReactionsError}</p>}
        <label className={styles.checkboxLabel}>
          <input
            type="checkbox"
            checked={showQuickReactions}
            disabled={quickReactionsSaving}
            onChange={(e) => toggleQuickReactions(e.target.checked)}
          />
          {t("account:appearanceSettings.quickReactionsLabel")}
        </label>
      </div>

      <h2 className={styles.groupTitle}>
        {t("account:appearanceSettings.deviceGroupTitle")}
      </h2>

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
          {t("account:appearanceSettings.dataSaverTitle")}
        </h3>
        <label className={styles.checkboxLabel}>
          <input
            type="checkbox"
            checked={dataSaver}
            onChange={(e) => setDataSaver(e.target.checked)}
          />
          {t("account:appearanceSettings.dataSaverLabel")}
        </label>
      </div>
    </>
  );

  return <AppShell center={center} />;
}
