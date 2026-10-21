import { useState } from "react";
import { useTranslation } from "react-i18next";
import { getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { GuideKey, HIDEABLE_GUIDES, useHiddenGuides } from "../hooks/useHiddenGuides";
import styles from "./GuidesSettings.module.css";

/** 設定画面「おせっかいなガイドの非表示」。確認ダイアログの「今後は表示しない」で非表示に
 * したガイドを、ガイドごとのチェックで切り替える（チェックあり＝表示しない）。 */
export default function GuidesSettingsPage() {
  const { t } = useTranslation();
  const { isGuideHidden, setGuideHidden } = useHiddenGuides();
  const [pending, setPending] = useState<GuideKey | null>(null);
  const [error, setError] = useState("");

  async function onToggle(guide: GuideKey, hidden: boolean) {
    setPending(guide);
    setError("");
    try {
      await setGuideHidden(guide, hidden);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setPending(null);
    }
  }

  const center = (
    <div className={styles.section}>
      <h3 className={styles.sectionTitle}>{t("account:guidesSettings.title")}</h3>
      <p className={styles.description}>{t("account:guidesSettings.intro")}</p>
      {error && <p className={styles.error}>{error}</p>}
      {HIDEABLE_GUIDES.map((guide) => (
        <div key={guide} className={styles.guide}>
          <label className={styles.checkboxLabel}>
            <input
              type="checkbox"
              checked={isGuideHidden(guide)}
              disabled={pending === guide}
              onChange={(e) => onToggle(guide, e.target.checked)}
            />
            {t(`account:guidesSettings.guides.${guide}.title`)}
          </label>
          <p className={styles.description}>
            {t(`account:guidesSettings.guides.${guide}.description`)}
          </p>
        </div>
      ))}
    </div>
  );

  return <AppShell center={center} />;
}
