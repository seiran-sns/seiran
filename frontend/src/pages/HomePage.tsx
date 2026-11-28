import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ListSummary, Note } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import PostComposer from "../components/note/PostComposer";
import NotificationsPanel from "../components/right/NotificationsPanel";
import SearchPanel from "../components/right/SearchPanel";
import { useAuth } from "../contexts/AuthContext";
import { useRightPane } from "../contexts/RightPaneContext";
import { Feed, feedKey, useHomeFeed } from "../contexts/HomeFeedContext";
import { ChannelSpec, useStreamingContext } from "../contexts/StreamingContext";
import { useToast } from "../contexts/ToastContext";
import { useCursorPagination } from "../hooks/useCursorPagination";
import { useIsNarrowViewport } from "../hooks/useIsNarrowViewport";
import { type SlideSide, useSlideSwipe } from "../hooks/useSlideSwipe";
import { filterTimelineNotes } from "../lib/timelineVisibility";
import {
  applyTimelineScroll,
  captureTimelineScroll,
  TIMELINE_TOP,
  type TimelineScroll,
} from "../lib/timelineScroll";
import styles from "./HomePage.module.css";

const PAGE_SIZE = 30;
const COMPOSER_COLLAPSED_KEY = "seiran_composer_collapsed";
const LOCAL_MODERATION_KEY = "seiran_home_local_moderation";

function loadLocalModeration(): boolean {
  try {
    return localStorage.getItem(LOCAL_MODERATION_KEY) === "1";
  } catch {
    return false;
  }
}

function saveLocalModeration(on: boolean) {
  try {
    localStorage.setItem(LOCAL_MODERATION_KEY, on ? "1" : "0");
  } catch {
    // 保存できなくても表示の切り替え自体は効く（次回は既定のOFFに戻るだけ）。
  }
}

/** トップにいない間に届いた新しいポスト。トップへ戻ったときにまとめて一覧の先頭へ入れる。 */
interface PendingHead {
  notes: Note[];
  /** 復帰時の補完で、取得した先頭ページと一覧が重ならなかった（間に取りこぼしがある）。 */
  gap: boolean;
}

const NO_PENDING_HEAD: PendingHead = { notes: [], gap: false };

function newestFirst(a: Note, b: Note): number {
  return a.createdAt < b.createdAt ? 1 : a.createdAt > b.createdAt ? -1 : 0;
}

function fetchFeed(feed: Feed, params: { limit?: number; until_id?: string; since_id?: string }) {
  // DM（visibility="direct"）はタイムラインに一切現れない仕様のため、対応エンドポイントには
  // 常に exclude_direct を付与する（Misskey API互換のためデフォルトでは含まれるが、
  // seiranフロントエンドは明示的に除外を要求する）。
  const request = feed.kind === "home"
    ? api.notes.homeTimeline({ ...params, exclude_direct: true })
    : feed.kind === "local"
    ? api.notes.localTimeline({ ...params, exclude_direct: true, moderation: feed.moderation })
    : feed.kind === "social"
    ? api.notes.socialTimeline({ ...params, exclude_direct: true })
    : feed.kind === "global"
    ? api.notes.globalTimeline({ ...params, exclude_direct: true })
    : feed.kind === "list"
    ? api.lists.timeline(feed.id, params)
    : api.hashtags.timeline(feed.name, params);
  return request.then((notes) => filterTimelineNotes(feed, notes));
}

/** 隣のタブとして描くもの（スワイプ中だけ）。 */
interface FeedPreview {
  key: string;
  side: SlideSide;
  feed: Feed;
  notes: Note[];
  gapBeforeIds: Set<string>;
  loading: boolean;
  /** そのタブで覚えている位置。 */
  scroll: TimelineScroll;
}

/** タブ（Feed）を対応するWebSocketタイムラインチャンネルへ変換する。 */
function feedToChannelSpec(feed: Feed): ChannelSpec {
  switch (feed.kind) {
    case "home":
      return { channel: "homeTimeline" };
    case "local":
      return feed.moderation ? { channel: "localTimelineModeration" } : { channel: "localTimeline" };
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
  const { feed: selectedFeed, setFeed, getCache, setCache } = useHomeFeed();
  const { user } = useAuth();
  const isAdmin = user?.role === "admin";
  // 管理者のモデレーション表示（ローカルTLにひかえめ・プライベートも混ぜる）。選択タブとは
  // 別に保存し、ローカルタブ選択中だけ実際のフィードに反映する。以降の`feed`は反映後のもので、
  // キャッシュ・取得API・購読チャンネルが通常のローカルTLと分かれる。
  const [localModeration, setLocalModeration] = useState(loadLocalModeration);
  const effectiveFeed = useCallback(
    (f: Feed): Feed => (f.kind === "local" && isAdmin && localModeration ? { kind: "local", moderation: true } : f),
    [isAdmin, localModeration],
  );
  const feed = useMemo(() => effectiveFeed(selectedFeed), [effectiveFeed, selectedFeed]);
  const selectedFeedKey = feedKey(selectedFeed);
  const [lists, setLists] = useState<ListSummary[]>([]);
  const [pinnedHashtags, setPinnedHashtags] = useState<{ name: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const loadingRef = useRef(loading);
  loadingRef.current = loading;
  // 現在の`notes`がどのタブ（feedKey）の一覧か。タブ切替の直後は、新しいタブの一覧が
  // 復元・取得されるまで`notes`に前のタブの一覧が残る。この間に一覧をキャッシュへ書いたり
  // 補完フェッチの結果をマージしたりすると、別タブの投稿が混ざるため、これで判定する。
  const [notesFeedKey, setNotesFeedKey] = useState<string | null>(null);
  const notesFeedKeyRef = useRef(notesFeedKey);
  notesFeedKeyRef.current = notesFeedKey;
  const [enteringIds, setEnteringIds] = useState<Set<string>>(new Set());
  // このIDの直前に「取りこぼし区間」の区切り（二重波線）を表示する対象ノートID群。
  const [gapBeforeIds, setGapBeforeIds] = useState<Set<string>>(new Set());
  const [composerCollapsed, setComposerCollapsed] = useState(
    () => localStorage.getItem(COMPOSER_COLLAPSED_KEY) === "1"
  );
  const { timelineTab, setTimelineTab, notifPanelScroll, setNotifPanelScroll, notifPanelCache, setNotifPanelCache } =
    useRightPane();
  const rightPaneRef = useRef<HTMLDivElement>(null);
  // クイック通知欄の実際のスクロールコンテナは`.rightScroll`（AppShell側、この直下div=
  // rightPaneRefの親要素）。overflow-y: autoで独立スクロールするため、windowではなく
  // このコンテナのscrollTopを保存・復元する。
  const getNotifScrollContainer = useCallback(
    () => (rightPaneRef.current?.parentElement as HTMLElement | null) ?? null,
    []
  );
  const isNarrow = useIsNarrowViewport();
  const { subscribeChannel, unread } = useStreamingContext();
  const timers = useRef<number[]>([]);
  const feedTabsRef = useRef<HTMLDivElement>(null);
  const centerRef = useRef<HTMLDivElement>(null);
  // 横スワイプで動かす範囲（sliderRef）と、その中の表示中の一覧（trackRef）・隣のタブの一覧
  // （previewPaneRef）。スクロール位置は表示中の一覧のポストで読む（隣のタブのポストも
  // 同じ data-timeline-id を持つため、centerRef 全体から探すと混ざる）。
  const sliderRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const previewPaneRef = useRef<HTMLDivElement>(null);

  // スクロール位置は「トップにいる」か「どのポストが一番上に見えているか」で持つ
  // （lib/timelineScroll.ts）。画面より上のポストの高さが変わったら（リアクション・画像の
  // 読み込み・新しいポスト）、一番上のポストを同じ位置へ戻す。新しいポストを一覧へ入れるのは
  // トップにいるときだけで、それ以外は pendingHead に溜めて件数を出す。
  const scrollRef = useRef<TimelineScroll>(TIMELINE_TOP);
  const [atTop, setAtTop] = useState(true);
  const [pendingHead, setPendingHead] = useState<PendingHead>(NO_PENDING_HEAD);
  const captureScroll = useCallback(
    () => captureTimelineScroll(trackRef.current, feedTabsRef.current?.getBoundingClientRect().bottom ?? 0),
    [],
  );
  // 位置を合わせた直後の window.scrollY。この値のままのスクロールイベントは自分で動かした
  // ものなので、ユーザーの位置として読まない（下の onScroll）。
  const pinnedScrollY = useRef<number | null>(null);
  const pinScroll = useCallback((scroll: TimelineScroll) => {
    const applied = applyTimelineScroll(trackRef.current, scroll);
    pinnedScrollY.current = window.scrollY;
    return applied;
  }, []);

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
  const currentFeedKeyRef = useRef(currentFeedKey);
  currentFeedKeyRef.current = currentFeedKey;

  const currentFeedIndex = availableFeeds.findIndex((f) => {
    if (f.kind !== feed.kind) return false;
    if (f.kind === "list") return f.id === (feed as { kind: "list"; id: string }).id;
    if (f.kind === "hashtag") return f.name === (feed as { kind: "hashtag"; name: string }).name;
    return true;
  });

  const neighborFeed = useCallback(
    (side: SlideSide): Feed | undefined =>
      currentFeedIndex < 0 ? undefined : availableFeeds[currentFeedIndex + side],
    [availableFeeds, currentFeedIndex],
  );

  // 選択中のタブを再度クリックした場合はタブ切替（実質no-op）ではなく先頭へスクロールする。
  const handleFeedTabClick = useCallback(
    (target: Feed) => {
      if (feedKey(target) === selectedFeedKey) {
        window.scrollTo({ top: 0, behavior: "smooth" });
        return;
      }
      setFeed(target);
    },
    [selectedFeedKey, setFeed],
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

  const onError = useCallback((err: unknown) => showError(getErrorMessage(err)), [showError]);
  const fetchPage = useCallback(
    (untilId: string) => fetchFeed(feed, { limit: PAGE_SIZE, until_id: untilId }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [feed, currentFeedKey]
  );
  const {
    items: notes,
    setItems: setNotes,
    hasMore,
    setHasMore,
    loadingMore,
    loadMore: loadMoreAny,
  } = useCursorPagination<Note>(fetchPage, (n) => n.id, PAGE_SIZE, onError, undefined, currentFeedKey);
  // 前のタブの一覧が残っている間に次ページを取ると、前のタブの末尾IDをカーソルにして
  // 新しいタブを取得し、そのまま前のタブの一覧へ追記してしまう。
  const loadMore = useCallback(() => {
    if (notesFeedKeyRef.current !== currentFeedKeyRef.current) return;
    loadMoreAny();
  }, [loadMoreAny]);
  const notesRef = useRef(notes);
  notesRef.current = notes;

  useEffect(() => {
    api.lists.list().then(setLists).catch(() => {});
    api.hashtags.pinned().then(setPinnedHashtags).catch(() => {});
  }, []);

  // スクロール位置は継続的に（都度）キャッシュへ書き戻す。「離脱時/アンマウント時に一度だけ
  // 捕捉する」方式は、React 18 StrictMode（開発時）が疑似アンマウントでeffectのcleanupを
  // 前倒しに発火させるため、まだ何もスクロールしていない新しいコンポーネントインスタンスの
  // 初期値で直前の復元値を上書きしてしまう。rAFへ遅延すると、画面遷移時にcleanupでcancel
  // されて最終位置を失う。復元待ちの間は書かない（復元前の短い一覧でブラウザが詰めた位置を
  // 拾って、覚えていた位置を上書きしてしまう）。
  useEffect(() => {
    const key = feedKey(feed);
    const onScroll = () => {
      if (pendingScrollRestore.current) return;
      // 離れるとき、DOM が外れてからこのリスナーが外れるまでの間にスクロールイベントが来る
      // （次の画面の描画で高さが変わる）。一覧が無い状態で読むと「トップ」になり、覚えていた
      // 位置を上書きする。リンクを押した時点で読むのを止める方式にはしない: 次の画面の
      // コードを読み込む間は遷移が保留され、この画面が表示されたままになる。その間に
      // ブラウザバックすると遷移が取り消されてこの画面がそのまま残り、止めたままになる。
      if (!centerRef.current?.isConnected) return;
      // 自分で位置を合わせたスクロールのイベントは、届くまでの間に画面より上の高さが変わって
      // いることがある（遅れて描かれる中身等）。そこで読むと、上が伸びた分だけ別のポストを
      // 「一番上」と覚えてしまうので、読まずに覚えているポストへ合わせ直す。
      if (pinnedScrollY.current !== null && Math.abs(window.scrollY - pinnedScrollY.current) < 1) {
        if (scrollRef.current.kind === "anchor") pinScroll(scrollRef.current);
        return;
      }
      pinnedScrollY.current = null;
      const scroll = captureScroll();
      scrollRef.current = scroll;
      setCache(key, { scroll });
      setAtTop(scroll.kind === "top");
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentFeedKey, setCache, captureScroll, pinScroll]);

  // 復元待ちのスクロール位置（キャッシュヒット時のみセットされ、一覧の描画後に一度だけ使う）。
  const pendingScrollRestore = useRef<TimelineScroll | null>(null);
  // スワイプで切り替えたタブで合わせる位置。隣のタブはスワイプ中に描いた位置のまま見せたいので、
  // そのタブで覚えていた位置ではなく、描いた位置（placePreview）を使う。
  const swipeScroll = useRef<{ key: string; scroll: TimelineScroll } | null>(null);
  const takeSwipeScroll = (key: string): TimelineScroll | null => {
    const s = swipeScroll.current;
    swipeScroll.current = null;
    return s?.key === key ? s.scroll : null;
  };
  // スワイプ中に隣のタブ用に始めた取得。切り替えた時点でまだ終わっていなければ、取り直さず待つ。
  const previewFetches = useRef(new Map<string, Promise<Note[]>>());

  useEffect(() => {
    const key = feedKey(feed);
    const cached = getCache(key);
    setPendingHead(NO_PENDING_HEAD);
    // 他画面から戻ってきた・タブを行き来した際は、キャッシュがあればそれをそのまま復元し
    // 再フェッチしない（一覧が一瞬空になってスクロール位置がズレるのを防ぐ）。
    if (cached) {
      setNotes(cached.notes);
      setNotesFeedKey(key);
      setHasMore(cached.hasMore);
      setGapBeforeIds(cached.gapBeforeIds);
      setLoading(false);
      // 補完フェッチの結果を一覧へ入れるか溜めるかを、戻る位置で決めるので先に反映する。
      const scroll = takeSwipeScroll(key) ?? cached.scroll;
      pendingScrollRestore.current = scroll;
      scrollRef.current = scroll;
      setAtTop(scroll.kind === "top");
      // 離脱中に取りこぼした新着・状態変化を補うため、先頭ページ相当を再取得してマージする
      // （WS再接続時の補完と同じ処理、詳細は下記 mergeHeadIntoTimeline）。
      mergeHeadIntoTimeline();
      return;
    }

    let cancelled = false;
    // キャッシュもスクロール位置の記憶も無い（一度も表示していない）タブへ切り替えた場合、
    // 前のタブのスクロール位置をそのまま持ち越すと、新しい一覧が読み込まれる前後で
    // 見た目上の高さが変わりスマホでは右ペイン領域まで巻き込んでスクロールしてしまう。
    // 先頭へ明示的にリセットする。スワイプで切り替えたときは、スワイプ中に描いた位置
    // （一覧の先頭をタブのすぐ下に置く）へ、取得した一覧を描いた後で合わせる。
    const initialScroll = takeSwipeScroll(key);
    if (initialScroll) {
      pendingScrollRestore.current = initialScroll;
      scrollRef.current = initialScroll;
      setAtTop(initialScroll.kind === "top");
    } else {
      window.scrollTo(0, 0);
      scrollRef.current = TIMELINE_TOP;
      setAtTop(true);
    }
    setLoading(true);
    setHasMore(true);
    setGapBeforeIds(new Set());
    (previewFetches.current.get(key) ?? fetchFeed(feed, { limit: PAGE_SIZE }))
      .then((n) => {
        if (cancelled) return;
        setNotes(n);
        setNotesFeedKey(key);
        setHasMore(n.length >= PAGE_SIZE);
      })
      .catch((e) => !cancelled && onError(e))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentFeedKey, feed, getCache, onError, setHasMore, setNotes]);

  // 一覧・hasMoreが変わるたびキャッシュへ反映（scrollは触らずマージする）。
  // loading中（フェッチ中・キャッシュ復元処理の途中）は書き込まない: React 18 StrictMode
  // （開発時）はmount直後に同一レンダーのeffectを2回連続実行するため、setNotes等の
  // 更新がまだ反映されていない「更新前の古いnotes（空配列）」をこのeffectが読んでしまい、
  // 直前に復元/フェッチ中の正しいキャッシュを空データで上書きしてしまう。
  // loadingがfalseになる本当のコミット後の再実行まで書き込みを待つことで、
  // 常に確定した値だけをキャッシュへ反映する。
  // タブ切替直後のコミットでは、loadingはまだ前のタブのfalseのまま・notesも前のタブの
  // 一覧のままこのeffectが走る。notesFeedKeyで照合しないと、前のタブの一覧が新しいタブの
  // キャッシュとして保存され、取得完了前に別タブへ移って戻ると混ざった一覧が表示される。
  useEffect(() => {
    if (loading || notesFeedKey !== currentFeedKey) return;
    setCache(currentFeedKey, { notes, hasMore, gapBeforeIds });
  }, [notes, hasMore, gapBeforeIds, loading, notesFeedKey, currentFeedKey, setCache]);

  // キャッシュから復元した一覧がDOMへ反映された直後（描画前）に、一度だけ位置を戻す。
  // 戻す先のポストが無くなっていたら先頭へ。
  useLayoutEffect(() => {
    if (loading || pendingScrollRestore.current === null) return;
    const scroll = pendingScrollRestore.current;
    pendingScrollRestore.current = null;
    if (!pinScroll(scroll)) {
      window.scrollTo(0, 0);
      scrollRef.current = TIMELINE_TOP;
      setAtTop(true);
    }
  }, [loading, notes, pinScroll]);

  // 一覧が変わったら（ポストの削除・区切りの挿入等）、一番上に見えていたポストを同じ位置へ戻す。
  // そのポストが消えていたら、今見えている位置を覚え直す。
  useLayoutEffect(() => {
    if (pendingScrollRestore.current) return;
    const scroll = scrollRef.current;
    if (scroll.kind === "anchor" && !pinScroll(scroll)) {
      scrollRef.current = captureScroll();
    }
  }, [notes, gapBeforeIds, captureScroll, pinScroll]);

  // ブラウザバックで戻ったとき、ブラウザ標準のスクロール復元が離れたときの画素位置を再現して、
  // 上の位置合わせを上書きする（画面より上の高さが前回と違うとポストがずれる）。この画面の
  // 履歴項目だけ標準の復元を止める（scrollRestoration は表示中の履歴項目に対して効くので、
  // 離れた後のクリーンアップで次の画面の項目を auto に戻す）。
  useEffect(() => {
    if (!("scrollRestoration" in history)) return;
    history.scrollRestoration = "manual";
    return () => {
      history.scrollRestoration = "auto";
    };
  }, []);

  // 一覧の外での高さの変化（リアクションが付いた・画像が読み込まれた・投稿欄の開閉等）にも
  // 合わせる。ResizeObserver は描画の前に呼ばれるので、ずれた瞬間は画面に出ない。
  // ブラウザ標準のスクロールアンカー（overflow-anchor）は二重に補正するので、
  // .swipeContainer で切っている。
  useEffect(() => {
    const el = centerRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (pendingScrollRestore.current || !el.isConnected) return;
      const scroll = scrollRef.current;
      if (scroll.kind === "anchor") pinScroll(scroll);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [pinScroll]);

  useEffect(() => () => timers.current.forEach((t) => window.clearTimeout(t)), []);

  // 先頭へポストを入れる。gap は、入れるポストと今の一覧の間に取りこぼしがあることを示す。
  const insertHead = useCallback((head: Note[], gap: boolean) => {
    const prev = notesRef.current;
    if (gap && prev.length > 0) setGapBeforeIds((g) => new Set(g).add(prev[0].id));
    setNotes((p) => {
      const ids = new Set(p.map((n) => n.id));
      const fresh = head.filter((n) => !ids.has(n.id));
      return fresh.length === 0 ? p : [...fresh, ...p];
    });
  }, [setNotes]);

  // トップにいないときは一覧を変えずに溜める（読んでいる最中に上へ増えても困らないように）。
  const queueHead = useCallback((head: Note[], gap: boolean) => {
    if (scrollRef.current.kind === "top") {
      insertHead(head, gap);
      return;
    }
    setPendingHead((p) => {
      const known = new Set([...notesRef.current, ...p.notes].map((n) => n.id));
      const fresh = head.filter((n) => !known.has(n.id));
      if (fresh.length === 0 && (!gap || p.gap)) return p;
      return { notes: [...fresh, ...p.notes].sort(newestFirst), gap: p.gap || gap };
    });
  }, [insertHead]);

  // トップへ戻ったら、溜めていたポストを入れる。
  useEffect(() => {
    if (!atTop || pendingHead.notes.length === 0) return;
    insertHead(pendingHead.notes, pendingHead.gap);
    setPendingHead(NO_PENDING_HEAD);
  }, [atTop, pendingHead, insertHead]);

  const prepend = useCallback((note: Note, animate = false) => {
    if (scrollRef.current.kind !== "top") {
      queueHead([note], false);
      return;
    }
    insertHead([note], false);
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
  }, [insertHead, queueHead]);

  // 離脱中（他画面へ遷移・WebSocket切断）に取りこぼした新着・状態変化を補うため、
  // 先頭ページ相当を再取得して現在の一覧の先頭とマージする。復帰時（上記キャッシュ復元時）と
  // WS再接続時（下記subscribeChannelのonResync）の両方から共通で呼ばれる。
  // 取得中にタブを切り替えた場合、結果は切替前のタブのものなので捨てる。
  const mergeHeadIntoTimeline = useCallback(() => {
    const key = feedKey(feed);
    fetchFeed(feed, { limit: PAGE_SIZE })
      .then((fetched) => {
        if (currentFeedKeyRef.current !== key || notesFeedKeyRef.current !== key) return;
        if (fetched.length === 0) return;
        const prev = notesRef.current;
        if (prev.length === 0) {
          setNotes(fetched);
          return;
        }
        const prevIds = new Set(prev.map((n) => n.id));
        const overlapIndex = fetched.findIndex((n) => prevIds.has(n.id));
        // 取得した先頭ページと既存の一覧がまったく重ならない＝間に取りこぼしがある。
        // 境目に区切り（二重波線）を挟んで、取得できた分だけ先頭へ追加する。
        if (overlapIndex === -1) {
          queueHead(fetched, true);
          return;
        }
        const newOnes = fetched.slice(0, overlapIndex);
        if (newOnes.length > 0) queueHead(newOnes, false);
      })
      .catch(() => {
        // 復帰時・再接続時の補完フェッチはベストエフォート。失敗してもエラー表示はしない
        // （元々表示中の一覧はそのまま残るため、ユーザー体験上は無視して問題ない）。
      });
  }, [feed, queueHead, setNotes]);

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

  // 横スワイプ（docs/ui_spec.md 2.4g）。動き出した時点で隣のタブを描き、指に合わせて横へ動かす。
  // 隣のタブはキャッシュがあればそれを、無ければ先頭ページの取得を始めて届いたら描く。
  // WebSocket の購読は切り替えてから（上の subscribeChannel が feed の変化で張り直す）。
  const [preview, setPreview] = useState<FeedPreview | null>(null);
  const previewRef = useRef(preview);
  previewRef.current = preview;
  // 隣のタブを今描いている位置に相当するスクロール位置（切り替えたらこの位置へ合わせる）。
  const previewScroll = useRef<TimelineScroll>(TIMELINE_TOP);
  const committedKey = useRef<string | null>(null);

  const fetchPreview = useCallback(
    (target: Feed) => {
      const key = feedKey(target);
      const running = previewFetches.current.get(key);
      if (running) return running;
      const request = fetchFeed(target, { limit: PAGE_SIZE });
      previewFetches.current.set(key, request);
      request
        .then((n) => {
          // 切り替えたときにキャッシュから出せるよう入れておく。先に一覧が入っていれば触らない。
          if (!getCache(key)?.notes.length) setCache(key, { notes: n, hasMore: n.length >= PAGE_SIZE });
        })
        .catch(() => {
          // 隣の下描き用の取得はベストエフォート。切り替えた後の取得でエラーを出す。
        })
        .finally(() => {
          if (previewFetches.current.get(key) === request) previewFetches.current.delete(key);
        });
      return request;
    },
    [getCache, setCache],
  );

  const handleNeighborChange = useCallback(
    (side: SlideSide | null) => {
      const target = side === null ? undefined : neighborFeed(side);
      if (side === null || !target) {
        setPreview(null);
        return;
      }
      const shown = effectiveFeed(target);
      const key = feedKey(shown);
      const cached = getCache(key);
      if (cached && cached.notes.length > 0) {
        setPreview({ key, side, feed: shown, notes: cached.notes, gapBeforeIds: cached.gapBeforeIds, loading: false, scroll: cached.scroll });
        return;
      }
      setPreview({ key, side, feed: shown, notes: [], gapBeforeIds: new Set(), loading: true, scroll: cached?.scroll ?? TIMELINE_TOP });
      const settle = (notes: Note[]) =>
        setPreview((p) => (p?.key === key ? { ...p, notes, loading: false } : p));
      fetchPreview(shown).then(settle, () => settle([]));
    },
    [neighborFeed, effectiveFeed, getCache, fetchPreview],
  );

  const handleSwipeCommit = useCallback(
    (side: SlideSide) => {
      const target = neighborFeed(side);
      const p = previewRef.current;
      if (!target || !p) {
        finishSwipe();
        return;
      }
      committedKey.current = p.key;
      swipeScroll.current = { key: p.key, scroll: previewScroll.current };
      setFeed(target);
    },
    // finishSwipe は下の useSlideSwipe の戻り値（安定した関数）。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [neighborFeed, setFeed],
  );

  const {
    phase: swipePhase,
    onTouchStart: onSwipeTouchStart,
    syncPane,
    finish: finishSwipe,
  } = useSlideSwipe({
    sliderRef,
    trackRef,
    paneRef: previewPaneRef,
    hasNeighbor: (side) => neighborFeed(side) !== undefined,
    onNeighborChange: handleNeighborChange,
    onCommit: handleSwipeCommit,
  });

  // 隣のタブを縦のどこに描くか。覚えている位置が「このポストが一番上」なら、そのポストを
  // 覚えた位置に置く（切り替えた後に合わせる位置と同じなので、切り替えても動かない）。
  // それ以外は一覧の先頭を表示中の一覧の先頭と揃え、表示中の一覧がタブの下へ隠れるほど
  // 進んでいればタブのすぐ下に置く（そのタブで覚えていたスクロール量に戻すと、深く読み進めて
  // いるときは隣の一覧が画面の外になり、スワイプ中に何も見えない）。
  const placePreview = useCallback(() => {
    const pane = previewPaneRef.current;
    const slider = sliderRef.current;
    const p = previewRef.current;
    if (!pane || !slider || !p) return;
    const sliderTop = slider.getBoundingClientRect().top;
    if (p.scroll.kind === "anchor") {
      const el = pane.querySelector<HTMLElement>(`[data-timeline-id="${CSS.escape(p.scroll.id)}"]`);
      if (el) {
        const offset = el.getBoundingClientRect().top - pane.getBoundingClientRect().top;
        pane.style.top = `${p.scroll.top - sliderTop - offset}px`;
        previewScroll.current = p.scroll;
        return;
      }
    }
    const lift = Math.max(0, (feedTabsRef.current?.getBoundingClientRect().bottom ?? 0) - sliderTop);
    pane.style.top = `${lift}px`;
    previewScroll.current = { kind: "top", y: Math.max(0, window.scrollY - lift) };
  }, []);

  useLayoutEffect(() => {
    if (!preview) return;
    placePreview();
    syncPane();
  }, [preview, placePreview, syncPane]);

  // 隣のタブの中身の高さ（画像の読み込み等）や縦スクロールが変わったら置き直す。
  const previewShown = preview !== null;
  useEffect(() => {
    const pane = previewPaneRef.current;
    if (!previewShown || !pane) return;
    window.addEventListener("scroll", placePreview, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(placePreview);
    observer?.observe(pane);
    return () => {
      window.removeEventListener("scroll", placePreview);
      observer?.disconnect();
    };
  }, [previewShown, placePreview]);

  // 切り替えた先の一覧が出せたら（キャッシュから戻した・取得中の表示になった）、隣の下描きを
  // 消して一覧を元の位置へ戻す。それまでは前のタブの一覧が残っているので、画面外に置いたまま
  // 隣の下描きを見せておく。描画の前に行い、前のタブの一覧が一瞬見えないようにする。
  useLayoutEffect(() => {
    if (swipePhase !== "committed" || currentFeedKey !== committedKey.current) return;
    if (notesFeedKey !== currentFeedKey && !loading) return;
    committedKey.current = null;
    finishSwipe();
  }, [swipePhase, currentFeedKey, notesFeedKey, loading, finishSwipe]);

  function toggleComposerCollapsed() {
    setComposerCollapsed((prev) => {
      const next = !prev;
      localStorage.setItem(COMPOSER_COLLAPSED_KEY, next ? "1" : "0");
      return next;
    });
  }

  // 管理者のローカルTLのモデレーション表示の切り替え。ローカルTLの一覧の一部としてスワイプで
  // 一緒に動かすので、隣のタブの下描き（inPreview）にも同じものを描く（下描きは操作させない）。
  const moderationToggle = (inPreview: boolean) => (
    <label className={styles.moderationToggle}>
      <input
        type="checkbox"
        checked={localModeration}
        readOnly={inPreview}
        tabIndex={inPreview ? -1 : undefined}
        onChange={
          inPreview
            ? undefined
            : (e) => {
                setLocalModeration(e.target.checked);
                saveLocalModeration(e.target.checked);
              }
        }
      />
      {t("home:homePage.localModerationToggle")}
    </label>
  );

  const emptyMessageFor = (f: Feed) =>
    f.kind === "home"
      ? t("home:homePage.emptyHome")
      : f.kind === "local"
      ? t("home:homePage.emptyLocal")
      : f.kind === "social"
      ? t("home:homePage.emptySocial")
      : f.kind === "global"
      ? t("home:homePage.emptyGlobal")
      : f.kind === "hashtag"
      ? t("hashtag:hashtagPage.empty")
      : t("home:homePage.emptyList");

  const saveCurrentScroll = useCallback(() => {
    setCache(feedKey(feed), { scroll: captureScroll() });
  }, [feed, setCache, captureScroll]);

  const center = (
    <div className={styles.swipeContainer} ref={centerRef}>
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

      <div className={styles.feedTabs} ref={feedTabsRef}>
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

      {pendingHead.notes.length > 0 && (
        <div className={styles.newPostsBar}>
          <button
            type="button"
            className={styles.newPostsPill}
            onClick={() => window.scrollTo({ top: 0, behavior: "smooth" })}
          >
            {t("home:homePage.newPosts", { count: pendingHead.notes.length })}
          </button>
        </div>
      )}


      <div
        className={`${styles.slider} ${swipePhase !== "idle" ? styles.sliding : ""}`}
        ref={sliderRef}
        onTouchStart={onSwipeTouchStart}
      >
        <div ref={trackRef}>
          {selectedFeed.kind === "local" && isAdmin && moderationToggle(false)}
          <NoteList
            notes={notes}
            loading={loading}
            enteringIds={enteringIds}
            gapBeforeIds={gapBeforeIds}
            onLoadMore={loadMore}
            hasMore={hasMore}
            loadingMore={loadingMore}
            emptyMessage={emptyMessageFor(feed)}
          />
        </div>
        {preview && (
          <div
            className={styles.previewPane}
            ref={previewPaneRef}
            style={{ transform: `translateX(${preview.side * 100}%)` }}
            aria-hidden
          >
            {preview.feed.kind === "local" && isAdmin && moderationToggle(true)}
            <NoteList
              notes={preview.notes}
              loading={preview.loading}
              gapBeforeIds={preview.gapBeforeIds}
              emptyMessage={emptyMessageFor(preview.feed)}
            />
          </div>
        )}
      </div>
    </div>
  );

  // 選択中のタブシート（通知/検索）を再度クリックした場合はタブ切替ではなく、
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

  // 狭幅では右ペインが中央ペインの下に落ちる（AppShell.module.css）。中央ペインはタイムライン
  // の無限スクロールでどこまでも続くため、その状態では右ペインへ確実にたどり着く手段が無い。
  // にもかかわらずNotificationsPanelはマウント時点で通知を既読マークしてしまうため、実際には
  // 一度も見ていない通知が既読扱いになってしまう。狭幅では右ペイン自体を描画しない。
  const right = isNarrow ? undefined : (
    <div ref={rightPaneRef}>
      <Tabs
        tabs={[
          unread > 0 ? t("home:homePage.quickNotificationsWithCount", { count: unread }) : t("home:homePage.quickNotifications"),
          t("home:homePage.search"),
        ]}
        active={timelineTab}
        onChange={handleTimelineTabChange}
        sticky
        top={0}
      />
      {timelineTab === 0 ? (
        <NotificationsPanel
          scroll={notifPanelScroll}
          onScrollChange={setNotifPanelScroll}
          getScrollContainer={getNotifScrollContainer}
          cache={notifPanelCache}
          onCacheChange={setNotifPanelCache}
        />
      ) : (
        <SearchPanel />
      )}
    </div>
  );

  return <AppShell center={center} right={right} onPosted={prepend} onBeforeNavigate={saveCurrentScroll} />;
}
