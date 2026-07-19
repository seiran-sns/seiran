import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, Note } from "../api/client";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import { useComposer } from "../contexts/ComposerContext";
import panel from "../components/common/Panel.module.css";
import styles from "./HashtagPage.module.css";

const PAGE_SIZE = 30;

export default function HashtagPage() {
  const { t } = useTranslation();
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const { openCompose } = useComposer();

  const [notes, setNotes] = useState<Note[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [pinning, setPinning] = useState(false);
  const notesRef = useRef<Note[]>([]);
  const loadingMoreRef = useRef(false);
  notesRef.current = notes;

  const tagName = (name ?? "").toLowerCase();

  useEffect(() => {
    if (!tagName) return;
    let cancelled = false;
    setLoading(true);
    setHasMore(true);
    Promise.all([
      api.hashtags.timeline(tagName, { limit: PAGE_SIZE }),
      api.hashtags.pinned(),
    ])
      .then(([n, pinnedTags]) => {
        if (cancelled) return;
        setNotes(n);
        setHasMore(n.length >= PAGE_SIZE);
        setPinned(pinnedTags.some((p) => p.name === tagName));
      })
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [tagName]);

  const loadMore = useCallback(() => {
    if (!tagName || loadingMoreRef.current || notesRef.current.length === 0) return;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    const untilId = notesRef.current[notesRef.current.length - 1].id;
    api.hashtags
      .timeline(tagName, { limit: PAGE_SIZE, until_id: untilId })
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
  }, [tagName]);

  async function togglePin() {
    if (!tagName || pinning) return;
    setPinning(true);
    try {
      if (pinned) {
        await api.hashtags.unpin(tagName);
        setPinned(false);
      } else {
        await api.hashtags.pin(tagName);
        setPinned(true);
      }
    } finally {
      setPinning(false);
    }
  }

  function handleComposeWithTag() {
    openCompose(`#${tagName} `);
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={() => navigate(-1)}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>#{tagName}</span>
      </header>

      <div className={styles.actionsRow}>
        <button
          className={`${styles.actionBtn} ${pinned ? styles.actionBtnActive : ""}`}
          onClick={togglePin}
          disabled={pinning}
        >
          {pinned ? t("hashtag:hashtagPage.removeFromHome") : t("hashtag:hashtagPage.addToHome")}
        </button>
        <button className={`${styles.actionBtn} ${styles.postBtn}`} onClick={handleComposeWithTag}>
          {t("hashtag:hashtagPage.postWithTag")}
        </button>
      </div>

      <NoteList
        notes={notes}
        loading={loading}
        emptyMessage={t("hashtag:hashtagPage.empty")}
        onLoadMore={loadMore}
        hasMore={hasMore}
        loadingMore={loadingMore}
      />
    </>
  );

  return <AppShell center={center} />;
}
