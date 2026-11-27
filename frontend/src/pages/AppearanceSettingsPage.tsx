import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import SwitchField from "../components/common/SwitchField";
import { useAuth } from "../contexts/AuthContext";
import { useTheme, type ThemePreference } from "../contexts/ThemeContext";
import i18n, {
  normalizeDetectedLanguage,
  isDisplayLanguage,
  type DisplayLanguage,
} from "../i18n";
import { setDataSaver, useDataSaver } from "../stores/dataSaverStore";
import { setLowContrast, useLowContrast } from "../stores/lowContrastStore";
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

/** 設定画面「表示」（#55, #127）。アカウントに保存する言語・絵文字リアクション・ワンタッチリアクションバーの表示と、
 * 「この端末だけの設定」としてこの端末の `localStorage` に保存するテーマ・低コントラスト・ギガ節約モードを選択する。 */
export default function AppearanceSettingsPage() {
  const { t } = useTranslation();
  const { user, updateUser } = useAuth();
  const { preference: themePreference, setPreference: setThemePreference } =
    useTheme();
  const dataSaver = useDataSaver();
  const lowContrast = useLowContrast();

  const [selected, setSelected] = useState<LanguageOption>(
    isDisplayLanguage(user?.language_preference ?? "")
      ? (user?.language_preference as DisplayLanguage)
      : "auto",
  );
  /** サーバーに保存されている表示言語（`null` は自動）。選び直しはここからの差分で送る。 */
  const savedLanguage = useRef<string | null>(user?.language_preference ?? null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [quickReactionsSaving, setQuickReactionsSaving] = useState(false);
  const [quickReactionsError, setQuickReactionsError] = useState("");
  // この項目を返さない版のサーバーでは、従来どおり表示されている。
  const showQuickReactions = user?.show_quick_reactions ?? true;
  const [emojiReactionsSaving, setEmojiReactionsSaving] = useState(false);
  const [emojiReactionsError, setEmojiReactionsError] = useState("");
  const useEmojiReactions = user?.use_emoji_reactions ?? true;

  async function toggleEmojiReactions(enabled: boolean) {
    setEmojiReactionsSaving(true);
    setEmojiReactionsError("");
    try {
      await api.account.updateEmojiReactions(enabled);
      updateUser({ use_emoji_reactions: enabled });
    } catch (err) {
      setEmojiReactionsError(getErrorMessage(err));
    } finally {
      setEmojiReactionsSaving(false);
    }
  }

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
      const language = option === "auto" ? null : option;
      // 別の画面で変えられていたら保存せずエラーを出す（選んだ値は画面に残す）
      await api.account.updateLanguage({ from: savedLanguage.current, to: language });
      savedLanguage.current = language;
      updateUser({ language_preference: language });
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
          {t("account:appearanceSettings.emojiReactionsTitle")}
        </h3>
        {emojiReactionsError && <p className={styles.error}>{emojiReactionsError}</p>}
        <SwitchField
          checked={useEmojiReactions}
          disabled={emojiReactionsSaving}
          onChange={toggleEmojiReactions}
        >
          {t("account:appearanceSettings.emojiReactionsLabel")}
        </SwitchField>
      </div>

      {/* 絵文字リアクションを使わない間はワンタッチリアクションバーも出ないので、選べないことを示す。 */}
      <div
        className={`${styles.section} ${useEmojiReactions ? "" : styles.sectionDisabled}`}
        aria-disabled={!useEmojiReactions}
      >
        <h3 className={styles.sectionTitle}>
          {t("account:appearanceSettings.quickReactionsTitle")}
        </h3>
        {quickReactionsError && <p className={styles.error}>{quickReactionsError}</p>}
        <SwitchField
          checked={showQuickReactions}
          disabled={quickReactionsSaving || !useEmojiReactions}
          onChange={toggleQuickReactions}
        >
          {t("account:appearanceSettings.quickReactionsLabel")}
        </SwitchField>
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
          {t("account:appearanceSettings.lowContrastTitle")}
        </h3>
        <SwitchField checked={lowContrast} onChange={setLowContrast}>
          {t("account:appearanceSettings.lowContrastLabel")}
        </SwitchField>
      </div>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:appearanceSettings.dataSaverTitle")}
        </h3>
        <SwitchField checked={dataSaver} onChange={setDataSaver}>
          {t("account:appearanceSettings.dataSaverLabel")}
        </SwitchField>
      </div>
    </>
  );

  return <AppShell center={center} />;
}
