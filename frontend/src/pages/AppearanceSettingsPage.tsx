import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useAuth } from "../contexts/AuthContext";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import i18n from "../i18n";
import panel from "../components/common/Panel.module.css";
import styles from "./AppearanceSettings.module.css";

type LanguageOption = "auto" | "ja" | "en";

const LANGUAGE_LABEL_KEYS: Record<LanguageOption, string> = {
  auto: "appearanceSettings.languageAuto",
  ja: "appearanceSettings.languageJa",
  en: "appearanceSettings.languageEn",
};

/** ブラウザの言語設定から表示言語を推定する（「自動」選択時、`i18next-browser-languagedetector` の navigator 判定と同じ方針）。 */
function detectAutoLanguage(): string {
  const langs = navigator.languages && navigator.languages.length > 0 ? navigator.languages : [navigator.language];
  for (const lang of langs) {
    if (lang.toLowerCase().startsWith("ja")) return "ja";
  }
  return "en";
}

/** 設定画面「表示」＞「言語」（#55）。自動 / 日本語 / 英語から選択し、サーバーに保存する。 */
export default function AppearanceSettingsPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const goBack = useGoBack();

  const [selected, setSelected] = useState<LanguageOption>(
    user?.language_preference === "ja" || user?.language_preference === "en" ? user.language_preference : "auto"
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
        <span className={panel.title}>{t("account:appearanceSettings.title")}</span>
      </header>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("account:appearanceSettings.languageTitle")}</h3>
        <p className={styles.hint}>{t("account:appearanceSettings.languageHint")}</p>
        {error && <p className={styles.error}>{error}</p>}
        <div className={styles.options}>
          {(Object.keys(LANGUAGE_LABEL_KEYS) as LanguageOption[]).map((option) => (
            <label key={option} className={styles.option}>
              <input
                type="radio"
                name="language"
                checked={selected === option}
                disabled={saving}
                onChange={() => selectLanguage(option)}
              />
              {t(`account:${LANGUAGE_LABEL_KEYS[option]}`)}
            </label>
          ))}
        </div>
        {saved && <p className={styles.success}>{t("account:appearanceSettings.saved")}</p>}
      </div>
    </>
  );

  return <AppShell center={center} />;
}
