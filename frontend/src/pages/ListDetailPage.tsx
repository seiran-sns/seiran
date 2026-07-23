import { useCallback, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, ApiError, getErrorMessage, ListDetail, Note } from "../api/client";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useToast } from "../contexts/ToastContext";
import { useCursorPagination } from "../hooks/useCursorPagination";
import panel from "../components/common/Panel.module.css";
import styles from "./ListDetailPage.module.css";

const PAGE_SIZE = 30;

export default function ListDetailPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id: string }>();
  const goBack = useGoBack();
  const { showError } = useToast();

  const [detail, setDetail] = useState<ListDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState("");

  const onError = useCallback((e: unknown) => showError(getErrorMessage(e)), [showError]);
  const fetchPage = useCallback(
    (untilId: string) => api.lists.timeline(id as string, { limit: PAGE_SIZE, until_id: untilId }),
    [id]
  );
  const { items: notes, setItems: setNotes, hasMore, setHasMore, loadingMore, loadMore } = useCursorPagination<Note>(
    fetchPage,
    (n) => n.id,
    PAGE_SIZE,
    onError
  );

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setLoading(true);
    setHasMore(true);
    setNotFound(false);
    setError("");
    Promise.all([api.lists.get(id), api.lists.timeline(id, { limit: PAGE_SIZE })])
      .then(([d, n]) => {
        if (cancelled) return;
        setDetail(d);
        setNotes(n);
        setHasMore(n.length >= PAGE_SIZE);
      })
      .catch((e) => {
        if (cancelled) return;
        if (e instanceof ApiError && e.code === "LIST_NOT_FOUND") {
          setNotFound(true);
        } else {
          setError(getErrorMessage(e));
        }
      })
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{detail ? detail.name : t("lists:listDetailPage.title")}</span>
      </header>

      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : notFound ? (
        <p className={panel.message}>{t("lists:listDetailPage.notFound")}</p>
      ) : error ? (
        <p className={panel.message}>{error}</p>
      ) : detail ? (
        <>
          <div className={styles.meta}>
            <span>{t("lists:listDetailPage.memberCount", { count: detail.members.length })}</span>
            {detail.is_owner && (
              <Link to="/settings/lists" className={styles.editLink}>
                {t("common:edit")}
              </Link>
            )}
          </div>

          <div className={styles.membersRow}>
            {detail.members.map((m) => (
              <span key={m.actor_id} className={styles.memberBadge}>
                {m.avatar_url ? <img src={m.avatar_url} alt="" /> : <span>{(m.display_name || m.username)[0]?.toUpperCase()}</span>}
                {m.display_name || m.username}
              </span>
            ))}
          </div>

          <NoteList
            notes={notes}
            emptyMessage={t("lists:listDetailPage.emptyTimeline")}
            onLoadMore={loadMore}
            hasMore={hasMore}
            loadingMore={loadingMore}
          />
        </>
      ) : null}
    </>
  );

  return <AppShell center={center} />;
}
