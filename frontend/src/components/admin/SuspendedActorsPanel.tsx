import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, SuspendedActor, getErrorMessage } from "../../api/client";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import EmojiText from "../note/EmojiText";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

const PAGE_SIZE = 30;

/** 管理者・モデレーター向け「凍結済みユーザー」タブ。ローカル・リモート混在の
 * 凍結済みアクター一覧と凍結解除を提供する（#凍結リモート対応）。 */
export default function SuspendedActorsPanel() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);

  const fetchPage = useCallback(
    (afterId: string) => api.admin.listSuspendedActors({ afterId, limit: PAGE_SIZE }),
    []
  );
  const {
    items: actors,
    setItems: setActors,
    hasMore,
    setHasMore,
    loadingMore,
    loadMore,
  } = useCursorPagination<SuspendedActor>(
    fetchPage,
    (a) => a.id,
    PAGE_SIZE,
    (e) => setError(getErrorMessage(e))
  );
  const sentinelRef = useInfiniteScrollSentinel<HTMLDivElement>(loadMore, hasMore);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");
    api.admin
      .listSuspendedActors({ limit: PAGE_SIZE })
      .then((rows) => {
        if (cancelled) return;
        setActors(rows);
        setHasMore(rows.length >= PAGE_SIZE);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function unsuspend(a: SuspendedActor) {
    setBusyId(a.id);
    setError("");
    try {
      await api.admin.unsuspendActor(a.id);
      setActors((prev) => prev.filter((x) => x.id !== a.id));
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:suspendedActors.title")}</h2>
      {error && <p className={styles.error}>{error}</p>}
      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : (
        <div className={styles.card}>
          {actors.length === 0 && <p className={panel.message}>{t("admin:suspendedActors.emptyMessage")}</p>}
          {actors.map((a) => (
            <div key={a.id} className={styles.row}>
              <div className={styles.avatar}>
                {a.avatar_url ? (
                  <img src={a.avatar_url} alt="" />
                ) : (
                  <span>{(a.display_name || a.username)[0]?.toUpperCase() ?? "?"}</span>
                )}
              </div>
              <div className={styles.grow}>
                <div className={styles.primaryText}>
                  <EmojiText text={a.display_name || a.username} emojis={undefined} />
                </div>
                <div className={styles.subText}>
                  @{a.username}
                  {a.domain ? `@${a.domain}` : ""}
                  {a.email ? ` · ${a.email}` : ""}
                </div>
              </div>
              <span className={`${styles.badge} ${a.user_id ? styles.badgeAdmin : ""}`}>
                {a.user_id
                  ? t("admin:suspendedActors.localBadge")
                  : t("admin:suspendedActors.remoteBadge")}
              </span>
              <button
                className={styles.btnGhost}
                disabled={busyId === a.id}
                onClick={() => unsuspend(a)}
              >
                {t("admin:suspendedActors.unsuspendButton")}
              </button>
            </div>
          ))}
        </div>
      )}
      {hasMore && (
        <div ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </div>
      )}
    </div>
  );
}
