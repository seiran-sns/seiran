import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { api, FollowListItem, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import Avatar from "../components/note/Avatar";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useStreamingContext } from "../contexts/StreamingContext";
import { useToast } from "../contexts/ToastContext";
import { profilePath } from "../lib/format";
import panel from "../components/common/Panel.module.css";
import styles from "./FollowRequestsSettings.module.css";

/** 設定画面「承認待ちフォロー」（承認制フォロー中のみメニューに出現）。
 * 一覧の【承認】/【拒否】操作を行う。実際のATPコミット/AP Accept・Reject送信は
 * バックエンド（`follow_approval`）が担う。 */
export default function FollowRequestsSettingsPage() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const goBack = useGoBack();
  const { refreshFollowRequestCount } = useStreamingContext();

  const [items, setItems] = useState<FollowListItem[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [actionLoadingId, setActionLoadingId] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    api.followRequests
      .list()
      .then(setItems)
      .catch((e) => showError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, [showError]);

  useEffect(() => {
    load();
  }, [load]);

  async function accept(item: FollowListItem) {
    setActionLoadingId(item.actor_id);
    try {
      await api.followRequests.accept(item.actor_id);
      setItems((prev) => prev?.filter((a) => a.actor_id !== item.actor_id) ?? null);
      refreshFollowRequestCount();
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setActionLoadingId(null);
    }
  }

  async function reject(item: FollowListItem) {
    setActionLoadingId(item.actor_id);
    try {
      await api.followRequests.reject(item.actor_id);
      setItems((prev) => prev?.filter((a) => a.actor_id !== item.actor_id) ?? null);
      refreshFollowRequestCount();
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setActionLoadingId(null);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:followRequestsSettings.title")}</span>
      </header>

      {loading && <p className={panel.message}>{t("common:loading")}</p>}
      {!loading && items && items.length === 0 && (
        <p className={panel.message}>{t("account:followRequestsSettings.empty")}</p>
      )}

      {!loading && items && items.length > 0 && (
        <ul className={styles.list}>
          {items.map((item) => (
            <li key={item.actor_id} className={styles.row}>
              <Link to={profilePath(item.username, item.domain)} className={styles.actorLink}>
                <Avatar url={item.avatar_url} name={item.display_name || item.username} size={40} />
                <div className={styles.names}>
                  <span className={styles.displayName}>{item.display_name || item.username}</span>
                  <span className={styles.acct}>
                    @{item.username}
                    {item.domain && item.domain !== window.location.hostname && `@${item.domain}`}
                  </span>
                </div>
              </Link>
              <div className={styles.actions}>
                <button
                  type="button"
                  className={styles.acceptBtn}
                  disabled={actionLoadingId === item.actor_id}
                  onClick={() => accept(item)}
                >
                  {t("account:followRequestsSettings.acceptButton")}
                </button>
                <button
                  type="button"
                  className={styles.rejectBtn}
                  disabled={actionLoadingId === item.actor_id}
                  onClick={() => reject(item)}
                >
                  {t("account:followRequestsSettings.rejectButton")}
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </>
  );

  return <AppShell center={center} />;
}
