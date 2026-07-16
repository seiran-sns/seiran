import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api, ListSummary, Note } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import PostComposer from "../components/note/PostComposer";
import NotificationsPanel from "../components/right/NotificationsPanel";
import TrendsSearchPanel from "../components/right/TrendsSearchPanel";
import { useRightPane } from "../contexts/RightPaneContext";
import { useStreamingContext } from "../contexts/StreamingContext";
import panel from "../components/common/Panel.module.css";
import styles from "./HomePage.module.css";

const PAGE_SIZE = 30;

type Feed = { kind: "home" } | { kind: "local" } | { kind: "list"; id: string };

function feedKey(feed: Feed): string {
  return feed.kind === "list" ? `list:${feed.id}` : feed.kind;
}

function fetchFeed(feed: Feed, params: { limit?: number; until_id?: string; since_id?: string }) {
  return feed.kind === "home"
    ? api.notes.homeTimeline(params)
    : feed.kind === "local"
    ? api.notes.localTimeline(params)
    : api.lists.timeline(feed.id, params);
}

export default function HomePage() {
  const [feed, setFeed] = useState<Feed>({ kind: "home" });
  const [lists, setLists] = useState<ListSummary[]>([]);
  const [notes, setNotes] = useState<Note[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [enteringIds, setEnteringIds] = useState<Set<string>>(new Set());
  const { timelineTab, setTimelineTab } = useRightPane();
  const { registerNote, unread } = useStreamingContext();
  const timers = useRef<number[]>([]);
  const notesRef = useRef<Note[]>([]);
  const loadingMoreRef = useRef(false);
  notesRef.current = notes;

  useEffect(() => {
    api.lists.list().then(setLists).catch(() => {});
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setHasMore(true);
    fetchFeed(feed, { limit: PAGE_SIZE })
      .then((n) => {
        if (cancelled) return;
        setNotes(n);
        setHasMore(n.length >= PAGE_SIZE);
      })
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [feedKey(feed)]);

  const loadMore = useCallback(() => {
    if (loadingMoreRef.current || notesRef.current.length === 0) return;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    const untilId = notesRef.current[notesRef.current.length - 1].id;
    fetchFeed(feed, { limit: PAGE_SIZE, until_id: untilId })
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [feedKey(feed)]);

  useEffect(() => () => timers.current.forEach((t) => window.clearTimeout(t)), []);

  function prepend(note: Note, animate = false) {
    setNotes((prev) => (prev.some((n) => n.id === note.id) ? prev : [note, ...prev]));
    if (animate) {
      setEnteringIds((prev) => new Set(prev).add(note.id));
      const t = window.setTimeout(() => {
        setEnteringIds((prev) => {
          const next = new Set(prev);
          next.delete(note.id);
          return next;
        });
      }, 450);
      timers.current.push(t);
    }
  }

  // リアルタイム更新（#37）: ストリームで届いたポストをアニメ付きで先頭挿入。
  useEffect(() => registerNote((n) => prepend(n, true)), [registerNote]);

  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>ホーム</span>
      </header>

      <div className={styles.composerWrap}>
        <PostComposer onPosted={prepend} />
      </div>

      <div className={styles.feedTabs}>
        <button
          className={`${styles.feedTab} ${feed.kind === "home" ? styles.feedTabActive : ""}`}
          onClick={() => setFeed({ kind: "home" })}
        >
          ホーム
        </button>
        <button
          className={`${styles.feedTab} ${feed.kind === "local" ? styles.feedTabActive : ""}`}
          onClick={() => setFeed({ kind: "local" })}
        >
          ローカル
        </button>
        {lists.map((l) => (
          <button
            key={l.id}
            className={`${styles.feedTab} ${feed.kind === "list" && feed.id === l.id ? styles.feedTabActive : ""}`}
            onClick={() => setFeed({ kind: "list", id: l.id })}
          >
            {l.name}
          </button>
        ))}
        <Link to="/settings/lists" className={styles.feedTab}>
          + リスト管理
        </Link>
      </div>

      <NoteList
        notes={notes}
        loading={loading}
        enteringIds={enteringIds}
        onLoadMore={loadMore}
        hasMore={hasMore}
        loadingMore={loadingMore}
        emptyMessage={
          feed.kind === "home"
            ? "フォロー中のユーザーの投稿がここに表示されます。"
            : feed.kind === "local"
            ? "まだ投稿がありません。最初の投稿をしてみましょう！"
            : "このリストのメンバーの投稿がここに表示されます。"
        }
      />
    </>
  );

  const right = (
    <>
      <Tabs
        tabs={[unread > 0 ? `クイック通知 (${unread})` : "クイック通知", "トレンド＆検索"]}
        active={timelineTab}
        onChange={setTimelineTab}
      />
      {timelineTab === 0 ? <NotificationsPanel /> : <TrendsSearchPanel />}
    </>
  );

  return <AppShell center={center} right={right} onPosted={prepend} />;
}
