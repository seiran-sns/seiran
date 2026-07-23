import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ListSummary, Note } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import PostComposer from "../components/note/PostComposer";
import NotificationsPanel from "../components/right/NotificationsPanel";
import TrendsSearchPanel from "../components/right/TrendsSearchPanel";
import { useRightPane } from "../contexts/RightPaneContext";
import { Feed, feedKey, useHomeFeed } from "../contexts/HomeFeedContext";
import { useStreamingContext } from "../contexts/StreamingContext";
import { useToast } from "../contexts/ToastContext";
import { useCursorPagination } from "../hooks/useCursorPagination";
import { useSwipe } from "../hooks/useSwipe";
import panel from "../components/common/Panel.module.css";
import styles from "./HomePage.module.css";

const PAGE_SIZE = 30;
const COMPOSER_COLLAPSED_KEY = "seiran_composer_collapsed";

function fetchFeed(feed: Feed, params: { limit?: number; until_id?: string; since_id?: string }) {
  // DM（visibility="direct"）はタイムラインに一切現れない仕様のため、対応エンドポイントには
  // 常に exclude_direct を付与する（Misskey API互換のためデフォルトでは含まれるが、
  // seiranフロントエンドは明示的に除外を要求する）。
  return feed.kind === "home"
    ? api.notes.homeTimeline({ ...params, exclude_direct: true })
    : feed.kind === "local"
    ? api.notes.localTimeline({ ...params, exclude_direct: true })
    : feed.kind === "list"
    ? api.lists.timeline(feed.id, params)
    : api.hashtags.timeline(feed.name, params);
}

export default function HomePage() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const { feed, setFeed, getCache, setCache } = useHomeFeed();
  const [lists, setLists] = useState<ListSummary[]>([]);
  const [pinnedHashtags, setPinnedHashtags] = useState<{ name: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const [enteringIds, setEnteringIds] = useState<Set<string>>(new Set());
  const [composerCollapsed, setComposerCollapsed] = useState(
    () => localStorage.getItem(COMPOSER_COLLAPSED_KEY) === "1"
  );
  const { timelineTab, setTimelineTab } = useRightPane();
  const { registerNote, unread } = useStreamingContext();
  const timers = useRef<number[]>([]);
  const headerRef = useRef<HTMLElement>(null);
  const feedTabsRef = useRef<HTMLDivElement>(null);
  const [headerHeight, setHeaderHeight] = useState(0);

  // 利用可能なフィードタブの配列（順序定義）
  const availableFeeds = useCallback((): Feed[] => {
    return [
      { kind: "home" },
      { kind: "local" },
      ...lists.map((l) => ({ kind: "list" as const, id: l.id })),
      ...pinnedHashtags.map((h) => ({ kind: "hashtag" as const, name: h.name })),
    ];
  }, [lists, pinnedHashtags])();

  const currentFeedIndex = availableFeeds.findIndex((f) => {
    if (f.kind !== feed.kind) return false;
    if (f.kind === "list") return f.id === (feed as { kind: "list"; id: string }).id;
    if (f.kind === "hashtag") return f.name === (feed as { kind: "hashtag"; name: string }).name;
    return true;
  });

  const handleSwipeLeft = useCallback(() => {
    if (currentFeedIndex >= 0 && currentFeedIndex < availableFeeds.length - 1) {
      setFeed(availableFeeds[currentFeedIndex + 1]);
    }
  }, [availableFeeds, currentFeedIndex, setFeed]);

  const handleSwipeRight = useCallback(() => {
    if (currentFeedIndex > 0) {
      setFeed(availableFeeds[currentFeedIndex - 1]);
    }
  }, [availableFeeds, currentFeedIndex, setFeed]);

  const swipeHandlers = useSwipe({
    onSwipeLeft: handleSwipeLeft,
    onSwipeRight: handleSwipeRight,
  });

  // フィード切り替え時にアクティブなタブ要素が見えるようにスクロール
  useEffect(() => {
    if (!feedTabsRef.current) return;
    const activeTabEl = feedTabsRef.current.querySelector<HTMLElement>(`.${styles.feedTabActive}`);
    if (activeTabEl) {
      activeTabEl.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "center" });
    }
  }, [feedKey(feed)]);

  // フィードタブ（下記feedTabs）はheaderの直下にstickyで張り付ける。両者とも
  // position: sticky; top: 0 だと重なってしまうため、headerの実高さ分だけオフセットする。
  useEffect(() => {
    const el = headerRef.current;
    if (!el) return;
    const update = () => setHeaderHeight(el.offsetHeight);
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const onError = useCallback((err: unknown) => showError(getErrorMessage(err)), [showError]);
  const fetchPage = useCallback(
    (untilId: string) => fetchFeed(feed, { limit: PAGE_SIZE, until_id: untilId }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [feedKey(feed)]
  );
  const { items: notes, setItems: setNotes, hasMore, setHasMore, loadingMore, loadMore } = useCursorPagination<Note>(
    fetchPage,
    (n) => n.id,
    PAGE_SIZE,
    onError
  );

  useEffect(() => {
    api.lists.list().then(setLists).catch(() => {});
    api.hashtags.pinned().then(setPinnedHashtags).catch(() => {});
  }, []);

  // スクロール位置は継続的に（都度）キャッシュへ書き戻す。「離脱時/アンマウント時に一度だけ
  // 捕捉する」方式は、React 18 StrictMode（開発時）が疑似アンマウントでeffectのcleanupを
  // 前倒しに発火させるため、まだ何もスクロールしていない新しいコンポーネントインスタンスの
  // 初期値（0）で直前の復元値を上書きしてしまう不具合があった（実機確認）。
  // rAFで間引きつつ書き込み、フィード切替のたびにこのeffect自体を張り替える。
  useEffect(() => {
    const key = feedKey(feed);
    let raf = 0;
    const onScroll = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        setCache(key, { scrollY: window.scrollY });
      });
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      if (raf) cancelAnimationFrame(raf);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [feedKey(feed)]);

  // 復元待ちのスクロール位置（キャッシュヒット時のみセットされ、一覧の描画後に一度だけ使う）。
  const pendingScrollRestore = useRef<number | null>(null);

  useEffect(() => {
    const key = feedKey(feed);
    const cached = getCache(key);
    // 他画面から戻ってきた・タブを行き来した際は、キャッシュがあればそれをそのまま復元し
    // 再フェッチしない（一覧が一瞬空になってスクロール位置がズレるのを防ぐ）。
    if (cached) {
      setNotes(cached.notes);
      setHasMore(cached.hasMore);
      setLoading(false);
      pendingScrollRestore.current = cached.scrollY;
      return;
    }

    let cancelled = false;
    setLoading(true);
    setHasMore(true);
    fetchFeed(feed, { limit: PAGE_SIZE })
      .then((n) => {
        if (cancelled) return;
        setNotes(n);
        setHasMore(n.length >= PAGE_SIZE);
      })
      .catch((e) => !cancelled && onError(e))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [feedKey(feed)]);

  // 一覧・hasMoreが変わるたびキャッシュへ反映（scrollYは触らずマージする）。
  // loading中（フェッチ中・キャッシュ復元処理の途中）は書き込まない: React 18 StrictMode
  // （開発時）はmount直後に同一レンダーのeffectを2回連続実行するため、setNotes等の
  // 更新がまだ反映されていない「更新前の古いnotes（空配列）」をこのeffectが読んでしまい、
  // 直前に復元/フェッチ中の正しいキャッシュを空データで上書きしてしまう不具合があった
  // （実機確認）。loadingがfalseになる本当のコミット後の再実行まで書き込みを待つことで、
  // 常に確定した値だけをキャッシュへ反映する。
  useEffect(() => {
    if (loading) return;
    setCache(feedKey(feed), { notes, hasMore });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [notes, hasMore, loading, feedKey(feed)]);

  // キャッシュから復元した一覧がDOMへ反映された後に、一度だけスクロール位置を復元する。
  useEffect(() => {
    if (loading || pendingScrollRestore.current === null) return;
    const y = pendingScrollRestore.current;
    pendingScrollRestore.current = null;
    requestAnimationFrame(() => window.scrollTo(0, y));
  }, [loading, notes]);

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

  function toggleComposerCollapsed() {
    setComposerCollapsed((prev) => {
      const next = !prev;
      localStorage.setItem(COMPOSER_COLLAPSED_KEY, next ? "1" : "0");
      return next;
    });
  }

  const center = (
    <div className={styles.swipeContainer} {...swipeHandlers}>
      <header className={panel.header} ref={headerRef}>
        <span className={panel.title}>{t("home:homePage.title")}</span>
      </header>

      <div className={styles.composerWrap}>
        <button
          type="button"
          className={styles.composerToggleBtn}
          onClick={toggleComposerCollapsed}
          aria-expanded={!composerCollapsed}
        >
          <span>{t("home:homePage.composerToggleLabel")}</span>
          <span className={styles.composerToggleIcon}>{composerCollapsed ? "▶" : "▼"}</span>
        </button>
        {!composerCollapsed && <PostComposer onPosted={prepend} />}
      </div>

      <div className={styles.feedTabs} ref={feedTabsRef} style={{ top: headerHeight }}>
        <button
          className={`${styles.feedTab} ${feed.kind === "home" ? styles.feedTabActive : ""}`}
          onClick={() => setFeed({ kind: "home" })}
        >
          {t("home:homePage.homeTab")}
        </button>
        <button
          className={`${styles.feedTab} ${feed.kind === "local" ? styles.feedTabActive : ""}`}
          onClick={() => setFeed({ kind: "local" })}
        >
          {t("home:homePage.localTab")}
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
          {t("home:homePage.manageListsLink")}
        </Link>
        {pinnedHashtags.map((h) => (
          <button
            key={h.name}
            className={`${styles.feedTab} ${feed.kind === "hashtag" && feed.name === h.name ? styles.feedTabActive : ""}`}
            onClick={() => setFeed({ kind: "hashtag", name: h.name })}
          >
            #{h.name}
          </button>
        ))}
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
            ? t("home:homePage.emptyHome")
            : feed.kind === "local"
            ? t("home:homePage.emptyLocal")
            : feed.kind === "hashtag"
            ? t("hashtag:hashtagPage.empty")
            : t("home:homePage.emptyList")
        }
      />
    </div>
  );

  const right = (
    <>
      <Tabs
        tabs={[
          unread > 0 ? t("home:homePage.quickNotificationsWithCount", { count: unread }) : t("home:homePage.quickNotifications"),
          t("home:homePage.trendsAndSearch"),
        ]}
        active={timelineTab}
        onChange={setTimelineTab}
      />
      {timelineTab === 0 ? <NotificationsPanel /> : <TrendsSearchPanel />}
    </>
  );

  return <AppShell center={center} right={right} onPosted={prepend} />;
}
