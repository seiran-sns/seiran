import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import type { ActivitySubscriptionAllowance, BlueskyPrivacy } from "../api/types";
import AppShell from "../components/layout/AppShell";
import SwitchField from "../components/common/SwitchField";
import { isEditConflict } from "../lib/editDiff";
import styles from "./PrivacySettings.module.css";

/** 設定画面「プライバシー」。Bsky Discoverフィード等のアルゴリズムレコメンドからの
 * 除外要求（`app.bsky.actor.contentVisibilityDeclaration`）、ログアウトしたユーザーへの
 * 非表示の要求（プロフィールの self-label `!no-unauthenticated`）、投稿の通知を許可する範囲
 * （`app.bsky.notification.declaration`）、承認制フォローを切り替える。 */

const ACTIVITY_SUBSCRIPTION_OPTIONS: ActivitySubscriptionAllowance[] = [
  "followers",
  "mutuals",
  "none",
];
export default function PrivacySettingsPage() {
  const { t } = useTranslation();

  const [hideFromAlgorithmicRecommendations, setHideFromAlgorithmicRecommendations] =
    useState(false);
  const [isLocked, setIsLocked] = useState(false);
  const [blueskyPrivacy, setBlueskyPrivacy] = useState<BlueskyPrivacy>({
    hide_from_unauthenticated: false,
    allow_activity_subscriptions: "followers",
  });
  const [blueskySaving, setBlueskySaving] = useState(false);
  /** サーバーに保存されている「投稿の通知」の範囲。ラジオボタンの選び直しはここからの差分で送る。 */
  const savedActivitySubscriptions = useRef<ActivitySubscriptionAllowance>("followers");
  /** 最後に保存できた項目（その項目の欄にだけ「保存しました」を出す）。 */
  const [blueskySaved, setBlueskySaved] = useState<keyof BlueskyPrivacy | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [lockSaving, setLockSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [lockSaved, setLockSaved] = useState(false);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.account.getContentVisibility(),
      api.account.getLock(),
      api.account.getBlueskyPrivacy(),
    ])
      .then(([contentVisibility, lock, bluesky]) => {
        if (!cancelled) {
          setHideFromAlgorithmicRecommendations(contentVisibility.hide_from_algorithmic_recommendations);
          setIsLocked(lock.is_locked);
          setBlueskyPrivacy(bluesky);
          savedActivitySubscriptions.current = bluesky.allow_activity_subscriptions;
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

  async function onUpdateBlueskyPrivacy(update: Partial<BlueskyPrivacy>) {
    const previous = blueskyPrivacy;
    setBlueskyPrivacy({ ...previous, ...update });
    setBlueskySaving(true);
    setError("");
    setBlueskySaved(null);
    try {
      const { allow_activity_subscriptions: allow, hide_from_unauthenticated } = update;
      const saved = await api.account.updateBlueskyPrivacy({
        hide_from_unauthenticated,
        // 別の画面で変えられていたら保存せずエラーを出す
        allow_activity_subscriptions:
          allow === undefined ? undefined : { from: savedActivitySubscriptions.current, to: allow },
      });
      savedActivitySubscriptions.current = saved.allow_activity_subscriptions;
      setBlueskyPrivacy(saved);
      setBlueskySaved(Object.keys(update)[0] as keyof BlueskyPrivacy);
    } catch (err) {
      // 別の画面との衝突なら、選んだ値は画面に残してエラーだけ出す
      if (!isEditConflict(err)) setBlueskyPrivacy(previous);
      setError(getErrorMessage(err));
    } finally {
      setBlueskySaving(false);
    }
  }

  async function onToggleLock(checked: boolean) {
    const previous = isLocked;
    setIsLocked(checked);
    setLockSaving(true);
    setError("");
    setLockSaved(false);
    try {
      await api.account.updateLock(checked);
      setLockSaved(true);
    } catch (err) {
      setIsLocked(previous);
      setError(getErrorMessage(err));
    } finally {
      setLockSaving(false);
    }
  }

  const center = (
    <>
      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:privacySettings.discoverabilityTitle")}
        </h3>
        {error && <p className={styles.error}>{error}</p>}
        <SwitchField
          checked={hideFromAlgorithmicRecommendations}
          disabled={loading || saving}
          onChange={onToggle}
        >
          {t("account:privacySettings.hideFromAlgorithmicRecommendationsLabel")}
        </SwitchField>
        {saved && (
          <p className={styles.success}>{t("account:privacySettings.saved")}</p>
        )}
      </div>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:privacySettings.loggedOutVisibilityTitle")}
        </h3>
        <SwitchField
          checked={blueskyPrivacy.hide_from_unauthenticated}
          disabled={loading || blueskySaving}
          onChange={(checked) => onUpdateBlueskyPrivacy({ hide_from_unauthenticated: checked })}
        >
          {t("account:privacySettings.hideFromUnauthenticatedLabel")}
        </SwitchField>
        <p className={styles.note}>{t("account:privacySettings.blueskyClientsOnlyNote")}</p>
        {blueskySaved === "hide_from_unauthenticated" && (
          <p className={styles.success}>{t("account:privacySettings.saved")}</p>
        )}
      </div>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>
          {t("account:privacySettings.activitySubscriptionsTitle")}
        </h3>
        <p className={styles.lead}>{t("account:privacySettings.activitySubscriptionsLabel")}</p>
        <div className={styles.radioGroup} role="radiogroup">
          {ACTIVITY_SUBSCRIPTION_OPTIONS.map((option) => (
            <label key={option} className={styles.radioLabel}>
              <input
                type="radio"
                name="allow_activity_subscriptions"
                value={option}
                checked={blueskyPrivacy.allow_activity_subscriptions === option}
                disabled={loading || blueskySaving}
                onChange={() => onUpdateBlueskyPrivacy({ allow_activity_subscriptions: option })}
              />
              {t(`account:privacySettings.activitySubscriptionsOptions.${option}`)}
            </label>
          ))}
        </div>
        <p className={styles.note}>{t("account:privacySettings.blueskyClientsOnlyNote")}</p>
        {blueskySaved === "allow_activity_subscriptions" && (
          <p className={styles.success}>{t("account:privacySettings.saved")}</p>
        )}
      </div>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("account:privacySettings.followApprovalTitle")}</h3>
        <SwitchField checked={isLocked} disabled={loading || lockSaving} onChange={onToggleLock}>
          {t("account:privacySettings.lockedAccountLabel")}
        </SwitchField>
        <p className={styles.description}>{t("account:privacySettings.lockedAccountDescription")}</p>
        {lockSaved && (
          <p className={styles.success}>{t("account:privacySettings.saved")}</p>
        )}
      </div>
    </>
  );

  return <AppShell center={center} />;
}
