import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, ApiError, getErrorMessage, ListDetail, Note } from "../api/client";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import panel from "../components/common/Panel.module.css";
import styles from "./ListDetailPage.module.css";

const PAGE_SIZE = 30;

export default function ListDetailPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [detail, setDetail] = useState<ListDetail | null>(null);
  const [notes, setNotes] = useState<Note[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState("");
  const notesRef = useRef<Note[]>([]);
  const loadingMoreRef = useRef(false);
  notesRef.current = notes;

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
  }, [id]);

  const loadMore = useCallback(() => {
    if (!id || loadingMoreRef.current || notesRef.current.length === 0) return;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    const untilId = notesRef.current[notesRef.current.length - 1].id;
    api.lists
      .timeline(id, { limit: PAGE_SIZE, until_id: untilId })
      .then((rows) => {
        setNotes((prev) => {
          const seen = new Set(prev.map((p) => p.id));
          const fresh = rows.filter((r) => !seen.has(r.id));
          return [...prev, ...fresh];
        });
        setHasMore(rows.length >= PAGE_SIZE);
      })
      .finally(() => {
        loadingMoreRef.current = false;
        setLoadingMore(false);
      });
  }, [id]);

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={() => navigate(-1)}>
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
