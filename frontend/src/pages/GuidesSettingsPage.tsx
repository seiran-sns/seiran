import { useState } from "react";
import { useTranslation } from "react-i18next";
import { getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import LabeledSwitch from "../components/common/LabeledSwitch";
import { GuideKey, HIDEABLE_GUIDES, useHiddenGuides } from "../hooks/useHiddenGuides";
import styles from "./GuidesSettings.module.css";

/** 設定画面「おせっかいなガイドの非表示」。確認ダイアログの「今後は表示しない」で非表示に
 * したガイドを、ガイドごとのスイッチ（表示する／表示しない）で切り替える。 */
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
      {error && <p className={styles.error}>{error}</p>}
      {HIDEABLE_GUIDES.map((guide) => (
        <div key={guide} className={styles.guide}>
          <div className={styles.guideText}>
            <span className={styles.guideTitle}>
              {t(`account:guidesSettings.guides.${guide}.title`)}
            </span>
            <p className={styles.description}>
              {t(`account:guidesSettings.guides.${guide}.description`)}
            </p>
          </div>
          <LabeledSwitch
            checked={!isGuideHidden(guide)}
            onChange={(shown) => onToggle(guide, !shown)}
            offLabel={t("account:guidesSettings.hide")}
            onLabel={t("account:guidesSettings.show")}
            ariaLabel={t(`account:guidesSettings.guides.${guide}.title`)}
            disabled={pending === guide}
          />
        </div>
      ))}
    </div>
  );

  return <AppShell center={center} />;
}
