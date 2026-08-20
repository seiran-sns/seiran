import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
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
import { ChannelSpec, useStreamingContext } from "../contexts/StreamingContext";
import { useToast } from "../contexts/ToastContext";
import { useCursorPagination } from "../hooks/useCursorPagination";
import { useSwipe } from "../hooks/useSwipe";
import { filterTimelineNotes } from "../lib/timelineVisibility";
import panel from "../components/common/Panel.module.css";
import styles from "./HomePage.module.css";

const PAGE_SIZE = 30;
const COMPOSER_COLLAPSED_KEY = "seiran_composer_collapsed";
const SCROLL_KEY_PREFIX = "seiran_home_scroll:";

function saveScrollPosition(key: string, y: number) {
  sessionStorage.setItem(`${SCROLL_KEY_PREFIX}${key}`, String(y));
}

function loadScrollPosition(key: string): number {
  const y = Number(sessionStorage.getItem(`${SCROLL_KEY_PREFIX}${key}`));
  return Number.isFinite(y) && y > 0 ? y : 0;
}

function fetchFeed(feed: Feed, params: { limit?: number; until_id?: string; since_id?: string }) {
  // DM（visibility="direct"）はタイムラインに一切現れない仕様のため、対応エンドポイントには
  // 常に exclude_direct を付与する（Misskey API互換のためデフォルトでは含まれるが、
  // seiranフロントエンドは明示的に除外を要求する）。
  const request = feed.kind === "home"
    ? api.notes.homeTimeline({ ...params, exclude_direct: true })
    : feed.kind === "local"
    ? api.notes.localTimeline({ ...params, exclude_direct: true })
    : feed.kind === "social"
    ? api.notes.socialTimeline({ ...params, exclude_direct: true })
    : feed.kind === "global"
    ? api.notes.globalTimeline({ ...params, exclude_direct: true })
    : feed.kind === "list"
    ? api.lists.timeline(feed.id, params)
    : api.hashtags.timeline(feed.name, params);
  return request.then((notes) => filterTimelineNotes(feed, notes));
}

/** タブ（Feed）を対応するWebSocketタイムラインチャンネルへ変換する。 */
function feedToChannelSpec(feed: Feed): ChannelSpec {
  switch (feed.kind) {
    case "home":
      return { channel: "homeTimeline" };
    case "local":
      return { channel: "localTimeline" };
    case "social":
      return { channel: "hybridTimeline" };
    case "global":
      return { channel: "globalTimeline" };
    case "list":
      return { channel: "userList", params: { listId: feed.id } };
    case "hashtag":
      return { channel: "hashtag", params: { tag: feed.name } };
  }
}

export default function HomePage() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const { feed, setFeed, getCache, setCache } = useHomeFeed();
  const [lists, setLists] = useState<ListSummary[]>([]);
  const [pinnedHashtags, setPinnedHashtags] = useState<{ name: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const loadingRef = useRef(loading);
  loadingRef.current = loading;
  const [enteringIds, setEnteringIds] = useState<Set<string>>(new Set());
  // このIDの直前に「取りこぼし区間」の区切り（二重波線）を表示する対象ノートID群。
  const [gapBeforeIds, setGapBeforeIds] = useState<Set<string>>(new Set());
  const [composerCollapsed, setComposerCollapsed] = useState(
    () => localStorage.getItem(COMPOSER_COLLAPSED_KEY) === "1"
  );
  const { timelineTab, setTimelineTab } = useRightPane();
  const rightPaneRef = useRef<HTMLDivElement>(null);
  const { subscribeChannel, unread } = useStreamingContext();
  const timers = useRef<number[]>([]);
  const navigatingAway = useRef(false);
  const headerRef = useRef<HTMLElement>(null);
  const feedTabsRef = useRef<HTMLDivElement>(null);
  const [headerHeight, setHeaderHeight] = useState(0);

  // 利用可能なフィードタブの配列（順序定義）
  const availableFeeds = useMemo(
    (): Feed[] => [
      { kind: "home" },
      { kind: "local" },
      { kind: "social" },
      { kind: "global" },
      ...lists.map((l) => ({ kind: "list" as const, id: l.id })),
      ...pinnedHashtags.map((h) => ({ kind: "hashtag" as const, name: h.name })),
    ],
    [lists, pinnedHashtags],
  );
  const currentFeedKey = feedKey(feed);

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

  // 選択中のタブを再度クリックした場合はタブ切替（実質no-op）ではなく先頭へスクロールする。
  const handleFeedTabClick = useCallback(
    (target: Feed) => {
      if (feedKey(target) === currentFeedKey) {
        window.scrollTo({ top: 0, behavior: "smooth" });
        return;
      }
      setFeed(target);
    },
    [currentFeedKey, setFeed],
  );

  // フィード切り替え時にアクティブなタブ要素が見えるようにスクロール
  useEffect(() => {
    const tabs = feedTabsRef.current;
    if (!tabs) return;
    const activeTabEl = tabs.querySelector<HTMLElement>(`.${styles.feedTabActive}`);
    if (activeTabEl) {
      // scrollIntoViewは横タブだけでなくwindowも縦に動かし、Home復帰時の
      // タイムラインスクロール復元を0へ戻してしまう。タブコンテナだけを横移動する。
      const left = activeTabEl.offsetLeft - (tabs.clientWidth - activeTabEl.offsetWidth) / 2;
      tabs.scrollTo({ left: Math.max(0, left), behavior: "smooth" });
    }
  }, [currentFeedKey]);

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
    [feed, currentFeedKey]
  );
  const { items: notes, setItems: setNotes, hasMore, setHasMore, loadingMore, loadMore } = useCursorPagination<Note>(
    fetchPage,
    (n) => n.id,
    PAGE_SIZE,
    onError
  );
  const notesRef = useRef(notes);
  notesRef.current = notes;

  useEffect(() => {
    api.lists.list().then(setLists).catch(() => {});
    api.hashtags.pinned().then(setPinnedHashtags).catch(() => {});
  }, []);

  // スクロール位置は継続的に（都度）キャッシュへ書き戻す。「離脱時/アンマウント時に一度だけ
  // 捕捉する」方式は、React 18 StrictMode（開発時）が疑似アンマウントでeffectのcleanupを
  // 前倒しに発火させるため、まだ何もスクロールしていない新しいコンポーネントインスタンスの
  // 初期値（0）で直前の復元値を上書きしてしまう不具合があった（実機確認）。
  // cacheはref内Mapへの軽量な同期書き込み（再renderなし）なので、scroll eventごとに
  // 即時保存する。rAFへ遅延すると、画面遷移時にcleanupでcancelされて最終位置を失う。
  useEffect(() => {
    const key = feedKey(feed);
    const onScroll = () => {
      if (navigatingAway.current) return;
      setCache(key, { scrollY: window.scrollY });
      saveScrollPosition(key, window.scrollY);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentFeedKey, setCache]);

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
      setGapBeforeIds(cached.gapBeforeIds);
      setLoading(false);
      pendingScrollRestore.current = Math.max(cached.scrollY, loadScrollPosition(key));
      // 離脱中に取りこぼした新着・状態変化を補うため、先頭ページ相当を再取得してマージする
      // （WS再接続時の補完と同じ処理、詳細は下記 mergeHeadIntoTimeline）。
      mergeHeadIntoTimeline();
      return;
    }

    let cancelled = false;
    setLoading(true);
    setHasMore(true);
    setGapBeforeIds(new Set());
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
  }, [currentFeedKey, feed, getCache, onError, setHasMore, setNotes]);

  // 一覧・hasMoreが変わるたびキャッシュへ反映（scrollYは触らずマージする）。
  // loading中（フェッチ中・キャッシュ復元処理の途中）は書き込まない: React 18 StrictMode
  // （開発時）はmount直後に同一レンダーのeffectを2回連続実行するため、setNotes等の
  // 更新がまだ反映されていない「更新前の古いnotes（空配列）」をこのeffectが読んでしまい、
  // 直前に復元/フェッチ中の正しいキャッシュを空データで上書きしてしまう不具合があった
  // （実機確認）。loadingがfalseになる本当のコミット後の再実行まで書き込みを待つことで、
  // 常に確定した値だけをキャッシュへ反映する。
  useEffect(() => {
    if (loading) return;
    setCache(currentFeedKey, { notes, hasMore, gapBeforeIds });
  }, [notes, hasMore, gapBeforeIds, loading, feed, currentFeedKey, setCache]);

  // キャッシュから復元した一覧がDOMへ反映された後に、一度だけスクロール位置を復元する。
  useEffect(() => {
    if (loading || pendingScrollRestore.current === null) return;
    const y = pendingScrollRestore.current;
    pendingScrollRestore.current = null;
    // 1回目のrAFではReactのDOM commit直後でscrollHeightが確定していないことがある。
    // 2 frame待って一覧レイアウト確定後に復元する。
    requestAnimationFrame(() => requestAnimationFrame(() => window.scrollTo(0, y)));
  }, [loading, notes]);

  useEffect(() => () => timers.current.forEach((t) => window.clearTimeout(t)), []);

  // スクロールが最上部にない状態で先頭挿入すると、増えた分だけ見えているポストが
  // 下へ押しやられてしまう。最上部でない場合は挿入前のscrollHeight/scrollYを控えておき、
  // DOM反映直後（下記useLayoutEffect）に増えた高さぶんだけscrollYを足して見た目を相殺する。
  const scrollAdjustRef = useRef<{ scrollHeight: number; scrollY: number } | null>(null);
  const captureScrollAdjust = useCallback(() => {
    if (window.scrollY > 0) {
      scrollAdjustRef.current = {
        scrollHeight: document.documentElement.scrollHeight,
        scrollY: window.scrollY,
      };
    }
  }, []);

  const prepend = useCallback((note: Note, animate = false) => {
    const preserveScroll = window.scrollY > 0;
    captureScrollAdjust();
    setNotes((prev) => (prev.some((n) => n.id === note.id) ? prev : [note, ...prev]));
    // push-downアニメーション（.entering、max-heightを0.4秒かけて展開）は高さがじわじわ
    // 伸びるため、非最上部でのスクロール補正（差分を一度に足し込む方式）と噛み合わない。
    // 最上部にいる時だけ演出する。
    if (animate && !preserveScroll) {
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
  }, [setNotes, captureScrollAdjust]);

  useLayoutEffect(() => {
    const adjust = scrollAdjustRef.current;
    scrollAdjustRef.current = null;
    if (!adjust) return;
    const diff = document.documentElement.scrollHeight - adjust.scrollHeight;
    if (diff !== 0) {
      window.scrollTo(0, adjust.scrollY + diff);
    }
  }, [notes]);

  // 離脱中（他画面へ遷移・WebSocket切断）に取りこぼした新着・状態変化を補うため、
  // 先頭ページ相当を再取得して現在の一覧の先頭とマージする。復帰時（上記キャッシュ復元時）と
  // WS再接続時（下記subscribeChannelのonResync）の両方から共通で呼ばれる。
  const mergeHeadIntoTimeline = useCallback(() => {
    fetchFeed(feed, { limit: PAGE_SIZE })
      .then((fetched) => {
        if (fetched.length === 0) return;
        const prev = notesRef.current;
        if (prev.length === 0) {
          setNotes(fetched);
          return;
        }
        const prevIds = new Set(prev.map((n) => n.id));
        const overlapIndex = fetched.findIndex((n) => prevIds.has(n.id));
        if (overlapIndex === -1) {
          // 取得した先頭ページと既存の一覧がまったく重ならない＝間に取りこぼしがある。
          // 境目に区切り（二重波線）を挟んで、取得できた分だけ先頭へ追加する。
          captureScrollAdjust();
          setGapBeforeIds((g) => new Set(g).add(prev[0].id));
          setNotes((p) => [...fetched, ...p]);
          return;
        }
        const newOnes = fetched.slice(0, overlapIndex);
        if (newOnes.length === 0) return;
        captureScrollAdjust();
        setNotes((p) => [...newOnes, ...p]);
      })
      .catch(() => {
        // 復帰時・再接続時の補完フェッチはベストエフォート。失敗してもエラー表示はしない
        // （元々表示中の一覧はそのまま残るため、ユーザー体験上は無視して問題ない）。
      });
  }, [feed, captureScrollAdjust, setNotes]);

  // リアルタイム更新（#37）: 表示中タブに対応するチャンネルを購読し、届いたポストを
  // アニメ付きで先頭挿入する。タブ切替のたびに旧チャンネルをdisconnectし新チャンネルへ
  // connectし直す（依存配列の`feed`変化でクリーンアップ→再購読される）。
  useEffect(() => {
    const spec = feedToChannelSpec(feed);
    return subscribeChannel(
      spec,
      (n) => {
        // バックエンドのチャンネル判定に加え、可視性（unlisted/followers_only）の
        // クライアント側最終防御をWS由来のノートにも適用する（RESTフェッチと同じ二重防御）。
        if (filterTimelineNotes(feed, [n]).length === 0) return;
        prepend(n, true);
      },
      () => {
        // WebSocketが不意に切断して再接続した場合の補完（初回接続時にも呼ばれるが、
        // その時点ではまだ何もフェッチしていないので二重フェッチを避けてスキップする）。
        if (loadingRef.current) return;
        mergeHeadIntoTimeline();
      }
    );
  }, [feed, subscribeChannel, prepend, mergeHeadIntoTimeline]);

  function toggleComposerCollapsed() {
    setComposerCollapsed((prev) => {
      const next = !prev;
      localStorage.setItem(COMPOSER_COLLAPSED_KEY, next ? "1" : "0");
      return next;
    });
  }

  const saveCurrentScroll = useCallback(() => {
    navigatingAway.current = true;
    const key = feedKey(feed);
    setCache(key, { scrollY: window.scrollY });
    saveScrollPosition(key, window.scrollY);
  }, [feed, setCache]);

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
          onClick={() => handleFeedTabClick({ kind: "home" })}
        >
          {t("home:homePage.homeTab")}
        </button>
        <button
          className={`${styles.feedTab} ${feed.kind === "local" ? styles.feedTabActive : ""}`}
          onClick={() => handleFeedTabClick({ kind: "local" })}
        >
          {t("home:homePage.localTab")}
        </button>
        <button
          className={`${styles.feedTab} ${feed.kind === "social" ? styles.feedTabActive : ""}`}
          onClick={() => handleFeedTabClick({ kind: "social" })}
        >
          {t("home:homePage.socialTab")}
        </button>
        <button
          className={`${styles.feedTab} ${feed.kind === "global" ? styles.feedTabActive : ""}`}
          onClick={() => handleFeedTabClick({ kind: "global" })}
        >
          {t("home:homePage.globalTab")}
        </button>
        {lists.map((l) => (
          <button
            key={l.id}
            className={`${styles.feedTab} ${feed.kind === "list" && feed.id === l.id ? styles.feedTabActive : ""}`}
            onClick={() => handleFeedTabClick({ kind: "list", id: l.id })}
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
            onClick={() => handleFeedTabClick({ kind: "hashtag", name: h.name })}
          >
            #{h.name}
          </button>
        ))}
      </div>

      <NoteList
        notes={notes}
        loading={loading}
        enteringIds={enteringIds}
        gapBeforeIds={gapBeforeIds}
        onLoadMore={loadMore}
        hasMore={hasMore}
        loadingMore={loadingMore}
        emptyMessage={
          feed.kind === "home"
            ? t("home:homePage.emptyHome")
            : feed.kind === "local"
            ? t("home:homePage.emptyLocal")
            : feed.kind === "social"
            ? t("home:homePage.emptySocial")
            : feed.kind === "global"
            ? t("home:homePage.emptyGlobal")
            : feed.kind === "hashtag"
            ? t("hashtag:hashtagPage.empty")
            : t("home:homePage.emptyList")
        }
      />
    </div>
  );

  // 選択中のタブシート（通知/トレンド・検索）を再度クリックした場合はタブ切替ではなく、
  // 右ペイン（PCではaside自体がoverflow-y:autoで独立スクロール、スマホでは通常のwindowスクロール
  // に合流する）の先頭へスクロールする。scrollIntoViewはどちらの場合も適切な祖先を辿って
  // 「このブロックの先頭」を画面上端に合わせてくれる。
  const handleTimelineTabChange = useCallback(
    (index: number) => {
      if (index === timelineTab) {
        rightPaneRef.current?.scrollIntoView({ behavior: "smooth", block: "start" });
        return;
      }
      setTimelineTab(index);
    },
    [timelineTab, setTimelineTab],
  );

  const right = (
    <div ref={rightPaneRef}>
      <Tabs
        tabs={[
          unread > 0 ? t("home:homePage.quickNotificationsWithCount", { count: unread }) : t("home:homePage.quickNotifications"),
          t("home:homePage.trendsAndSearch"),
        ]}
        active={timelineTab}
        onChange={handleTimelineTabChange}
        sticky
        top={0}
      />
      {timelineTab === 0 ? <NotificationsPanel /> : <TrendsSearchPanel />}
    </div>
  );

  return <AppShell center={center} right={right} onPosted={prepend} onBeforeNavigate={saveCurrentScroll} />;
}
