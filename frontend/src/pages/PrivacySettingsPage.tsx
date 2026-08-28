import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import panel from "../components/common/Panel.module.css";
import styles from "./PrivacySettings.module.css";

/** 設定画面「プライバシー」。Bsky Discoverフィード等のアルゴリズムレコメンドからの
 * 除外要求（`app.bsky.actor.contentVisibilityDeclaration`）を切り替える。 */
export default function PrivacySettingsPage() {
  const { t } = useTranslation();
  const goBack = useGoBack();

  const [hideFromAlgorithmicRecommendations, setHideFromAlgorithmicRecommendations] =
    useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api.account
      .getContentVisibility()
      .then((res) => {
        if (!cancelled) {
          setHideFromAlgorithmicRecommendations(res.hide_from_algorithmic_recommendations);
        }
      })
      .catch((err) => {
        if (!cancelled) setError(getErrorMessage(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function onToggle(checked: boolean) {
    const previous = hideFromAlgorithmicRecommendations;
    setHideFromAlgorithmicRecommendations(checked);
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      await api.account.updateContentVisibility(checked);
      setSaved(true);
    } catch (err) {
      setHideFromAlgorithmicRecommendations(previous);
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
        <span className={panel.title}>{t("account:privacySettings.title")}</span>
      </header>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:privacySettings.discoverabilityTitle")}
        </h3>
        {error && <p className={styles.error}>{error}</p>}
        <label className={styles.checkboxLabel}>
          <input
            type="checkbox"
            checked={hideFromAlgorithmicRecommendations}
            disabled={loading || saving}
            onChange={(e) => onToggle(e.target.checked)}
          />
          {t("account:privacySettings.hideFromAlgorithmicRecommendationsLabel")}
        </label>
        <p className={styles.description}>
          {t("account:privacySettings.hideFromAlgorithmicRecommendationsDescription")}
        </p>
        {saved && (
          <p className={styles.success}>{t("account:privacySettings.saved")}</p>
        )}
      </div>
    </>
  );

  return <AppShell center={center} />;
}
