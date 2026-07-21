import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, Note } from "../api/client";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import { useComposer } from "../contexts/ComposerContext";
import { useToast } from "../contexts/ToastContext";
import { useCursorPagination } from "../hooks/useCursorPagination";
import panel from "../components/common/Panel.module.css";
import styles from "./HashtagPage.module.css";

const PAGE_SIZE = 30;

export default function HashtagPage() {
  const { t } = useTranslation();
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const { openCompose } = useComposer();
  const { showError } = useToast();

  const [loading, setLoading] = useState(true);
  const [pinned, setPinned] = useState(false);
  const [pinning, setPinning] = useState(false);

  const tagName = (name ?? "").toLowerCase();

  const onError = useCallback((e: unknown) => showError(getErrorMessage(e)), [showError]);
  const fetchPage = useCallback(
    (untilId: string) => api.hashtags.timeline(tagName, { limit: PAGE_SIZE, until_id: untilId }),
    [tagName]
  );
  const { items: notes, setItems: setNotes, hasMore, setHasMore, loadingMore, loadMore } = useCursorPagination<Note>(
    fetchPage,
    (n) => n.id,
    PAGE_SIZE,
    onError
  );

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
      .catch((e) => !cancelled && onError(e))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
    } catch (err) {
      showError(getErrorMessage(err));
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
