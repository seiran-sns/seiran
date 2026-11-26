import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link, useNavigate } from "react-router-dom";
import i18n from "../../i18n";
import { api, getErrorMessage, NotificationItem, NotificationUser } from "../../api/client";
import { useAuth } from "../../contexts/AuthContext";
import { useStreamingContext } from "../../contexts/StreamingContext";
import { useToast } from "../../contexts/ToastContext";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import { profilePath, userAcct } from "../../lib/format";
import { parseReactionContent } from "../../lib/customEmojis";
import { isSelfUser, toRelationshipTarget } from "../../lib/userRelationship";
import RemoteImg from "../common/RemoteImg";
import panel from "../common/Panel.module.css";
import Avatar from "../note/Avatar";
import EmojiText from "../note/EmojiText";
import DisplayName from "../common/DisplayName";
import NoteHoverPreview from "../note/NoteHoverPreview";
import UserContextMenu from "../note/UserContextMenu";
import UserHoverArea from "../note/UserHoverArea";
import UserLinkTag from "../note/UserLinkTag";
import NotificationFilterBar, {
  EMPTY_NOTIFICATION_FILTER,
  NotificationFilterState,
  toNotificationQuery,
} from "./NotificationFilterBar";
import styles from "./NotificationsPanel.module.css";
import TwemojiEmoji from "../common/TwemojiEmoji";
import UiIcon from "../common/UiIcon";
import CardStack from "../common/CardStack";
import { groupNotifications } from "../../lib/notificationGroups";
import { applyTimelineScroll, captureTimelineScroll, TimelineScroll } from "../../lib/timelineScroll";

const PAGE_SIZE = 20;
/** 未読だった通知を目立たせておく時間（10秒＋元の背景へ戻る1.5秒。CSS の notifFresh と揃える）。 */
const FRESH_HIGHLIGHT_MS = 11_500;
/** 重ねた通知にマウスが乗っている間（と固定中）のずらし幅。1枚ずつめくりやすくする。 */
const DECK_SPREAD_OFFSET_PX = 32;

/** 2行で出す通知種別。2行目に対象ポストを出し、どのポストへの反応か・何と書かれたかを一目で
 * わかるようにする（返信・引用・メンションでは相手のポスト自体）。 */
const TWO_LINE_TYPES = new Set(["reaction", "renote", "reply", "quote", "mention"]);

/** 2行目に出す対象ポスト。リポスト通知の `note` は本文の無いラッパーなので、リポスト元を出す
 * （取り消し済みでラッパーが残っていなければ `note` がリポスト元そのもの）。返信・引用・
 * メンション通知の `note` は相手のポスト（返信・引用したポスト自体）。 */
function targetNote(n: NotificationItem) {
  return n.type === "renote" ? (n.note?.renote ?? n.note) : n.note;
}

/** 2行目: 対象ポストの CW 警告文（あれば）か本文を、改行を半角スペースでつないだ1行。 */
function targetSummary(n: NotificationItem): string {
  const note = targetNote(n);
  const cw = note?.cw?.trim();
  const source = cw ? cw : (note?.text ?? "");
  return source.replace(/\s*(\r?\n)+\s*/g, " ").trim();
}

/** ポストへのリンクを持つ通知種別（通知文全体を対象ポストへの遷移領域にする）。
 * `renote` は Misskey 本家の呼称（seiran内部では「リポスト」と呼ぶ処理と同じもの）。 */
const NOTE_LINKED_TYPES = new Set(["reaction", "mention", "reply", "renote", "quote"]);

/** 通知のダイジェスト表示・遷移先とすべきポストIDを求める。
 * "renote"（リポスト）通知の `note` は本文を持たないリポストラッパー投稿自体を指すため、
 * リポスト元の実体投稿（`note.renote`、`build_notes`/`embed_renotes` が埋め込む）を優先する。 */
export function resolveTargetNoteId(n: NotificationItem): string | undefined {
  if (!NOTE_LINKED_TYPES.has(n.type)) return undefined;
  const targetNote = n.type === "renote" ? (n.note?.renote ?? n.note) : n.note;
  return targetNote?.id;
}

/** 通知クリック時の遷移先とすべきポストID。
 * ダイジェスト表示・ホバープレビュー（`resolveTargetNoteId`）は "renote" 通知でも
 * リポスト元の実体投稿を優先するが、クリック遷移はリポストラッパー投稿自身
 * （`note.id`）へ飛ばす（リポストという行為そのものの投稿ページを開くため）。
 * リポストが取り消し・削除済み（`repostWithdrawn`）ならラッパーのページは開けないので
 * リポスト元へ飛ばす。 */
export function resolveClickTargetNoteId(n: NotificationItem): string | undefined {
  if (!NOTE_LINKED_TYPES.has(n.type)) return undefined;
  if (n.type === "renote" && n.repostWithdrawn) return resolveTargetNoteId(n);
  return n.note?.id;
}

/** 通知のユーザーの acct（`@user`・`@user@host`）。Bsky ユーザーは `host` が空文字で
 * `username` がハンドルそのもの（`alice.bsky.social`）なので `@handle` になる。 */
export function notificationUserAcct(u: NotificationUser | undefined): string | undefined {
  return u?.username ? userAcct(u.username, u.host) : undefined;
}

/** 通知1件を人間可読な文言に整形する。`iconUrl` があれば絵文字は画像（カスタム絵文字）。
 * `who`（表示名部分）は呼び出し側で `DisplayName` を通す前提でプレーンテキストのまま返す。
 * `useEmojiReactions` が偽（アカウント設定で絵文字リアクションを使わない）なら、リアクション通知は
 * 絵文字を問わず「❤️ いいねしました」にする（投稿カード側でもリアクションを ♥️ だけで扱うため）。 */
export function describeNotification(
  n: NotificationItem,
  useEmojiReactions = true,
): {
  icon: string;
  iconUrl?: string;
  i18nKey: string;
  who: string;
  whoEmojis?: Record<string, string>;
  /** 通知元ユーザーの acct。名前のツールチップに出す。 */
  whoAcct?: string;
  newHandle?: string;
} {
  const who = n.user?.name || n.user?.username || i18n.t("notifications:notificationsPanel.unknownUser");
  const whoAcct = notificationUserAcct(n.user);
  const whoEmojis = n.user?.emojis;
  // 引っ越し先は文中に出す。リモートなら acct、ローカル（host が null）なら名前。
  const newHandle =
    n.relatedUser?.host != null
      ? notificationUserAcct(n.relatedUser)
      : n.relatedUser?.name || n.relatedUser?.username;
  switch (n.type) {
    case "reaction": {
      if (!useEmojiReactions) {
        return { icon: "❤️", i18nKey: "notifications:notificationsPanel.likeText", who, whoEmojis, whoAcct };
      }
      // `reactionEmojis` のキーは Misskey 本家仕様に合わせコロンなし shortcode
      // （ローカルは `shortcode@.`、リモートは `shortcode@host`。バックエンド側 `convert.rs`）。
      // `reaction` は `:shortcode:`/`:shortcode@host:` 形式なので分解してから引く。
      const parsedReaction = n.reaction ? parseReactionContent(n.reaction) : null;
      const emojiKey = parsedReaction
        ? parsedReaction.shortcode + (parsedReaction.host ? `@${parsedReaction.host}` : "")
        : undefined;
      return {
        icon: n.reaction || "⭐",
        iconUrl: n.reactionEmojiUrl ?? (emojiKey ? n.note?.reactionEmojis?.[emojiKey] : undefined),
        i18nKey: "notifications:notificationsPanel.reactionText",
        who,
        whoEmojis,
        whoAcct,
      };
    }
    case "follow":
      return { icon: "➕", i18nKey: "notifications:notificationsPanel.followText", who, whoEmojis, whoAcct };
    // バックエンド`to_misskey_notification_type`が内部種別`followRequest`をMisskey本家の
    // `receiveFollowRequest`へ変換して返すため、ここでもその値を見る（`repost`→`renote`と同様）。
    case "receiveFollowRequest":
      return { icon: "🙋", i18nKey: "notifications:notificationsPanel.followRequestText", who, whoEmojis, whoAcct };
    case "followRequestAccepted":
      return { icon: "🤝", i18nKey: "notifications:notificationsPanel.followAcceptedText", who, whoEmojis, whoAcct };
    case "mention":
      return { icon: "📣", i18nKey: "notifications:notificationsPanel.mentionText", who, whoEmojis, whoAcct };
    case "reply":
      return { icon: "💬", i18nKey: "notifications:notificationsPanel.replyText", who, whoEmojis, whoAcct };
    case "renote":
      return { icon: "🔁", i18nKey: "notifications:notificationsPanel.repostText", who, whoEmojis, whoAcct };
    case "quote":
      return { icon: "❝", i18nKey: "notifications:notificationsPanel.quoteText", who, whoEmojis, whoAcct };
    case "moveRefollowed":
      return {
        icon: "🚚",
        i18nKey: "notifications:notificationsPanel.moveRefollowedText",
        who,
        whoEmojis,
        whoAcct,
        newHandle,
      };
    case "moveAlreadyFollowing":
      return {
        icon: "🚚",
        i18nKey: "notifications:notificationsPanel.moveAlreadyFollowingText",
        who,
        whoEmojis,
        whoAcct,
        newHandle,
      };
    default:
      return { icon: "🔔", i18nKey: "notifications:notificationsPanel.genericText", who, whoEmojis, whoAcct };
  }
}

/** 重ねた通知のまとまり1つの状態。戻ったときに同じ見た目で出すために覚える。 */
export interface NotificationDeckState {
  /** 前面に出していた通知の ID。 */
  front?: string;
  /** 広げたまま固定していたか（CardStack）。 */
  pinned?: boolean;
}

export interface NotificationsPanelCache {
  items: NotificationItem[];
  hasMore: boolean;
  /** 重ねるまとまりの境（`groupNotifications` の `baseOldestId`）。戻ったときに同じまとまりで出す。 */
  groupBaseId?: string;
  /** まとまりごとの状態（キーはまとまりの一番古い通知の ID）。マウスが乗るたびに変わるので、
   * 書き戻して呼び出し元を描き直させることはせず、このオブジェクトをその場で書き換える。
   * カードを押して画面を離れるときも、押した時点の状態がそのまま残る。 */
  decks?: Record<string, NotificationDeckState>;
}

interface NotificationsPanelProps {
  /** 復元すべきスクロール位置（一番上に見えている通知とその位置、lib/timelineScroll.ts）。
   * 省略時はスクロール位置の保存・復元を行わない。 */
  scroll?: TimelineScroll;
  /** スクロールのたびに現在位置を書き戻すコールバック。 */
  onScrollChange?: (scroll: TimelineScroll) => void;
  /** スクロールを監視・復元する要素を返す関数。省略時は`window`
   *（通知一覧画面のように中央ペインでwindowスクロールする場合）。
   * Home/Search画面の右ペインのようにコンテナ自身が`overflow-y: auto`で
   * 独立スクロールする場合はそのコンテナを返す関数を渡す。 */
  getScrollContainer?: () => HTMLElement | null;
  /** 前回表示時の一覧（追加読み込み分含む）。渡された場合は再フェッチせずこれを初期表示に使い、
   * 離脱中に届いた新着だけを補って差し込む。これが無いとスクロール位置だけ復元しても、
   * 再フェッチで先頭ページ分（`PAGE_SIZE`件）しか一覧が無い状態になり、無限スクロールで
   * それより深く読み込んでいた場合に一覧の実高さが足りずスクロール位置が正しく再現できない。 */
  cache?: NotificationsPanelCache;
  /** 一覧・hasMoreが変わるたびに呼び出し元へ書き戻すコールバック（上記`cache`の保存用）。 */
  onCacheChange?: (cache: NotificationsPanelCache) => void;
}

/**
 * ホーム右ペイン タブ2: クイック通知（Doc5 §2.1）。
 * `POST /api/i/notifications`（Misskey API 互換, Doc3 §5.5）で永続化された通知履歴を
 * 新着順に読み込み、下端までスクロールすると `untilId` カーソルで過去分を追加取得する。
 * WS 経由のライブ通知（`registerNotifArrived`）は「新着があった」というシグナルにのみ使い、
 * 実データは常に REST から取得することで、一覧表示と整合したID体系を保つ。
 */
export default function NotificationsPanel({
  scroll,
  onScrollChange,
  getScrollContainer,
  cache,
  onCacheChange,
}: NotificationsPanelProps = {}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { user: currentUser } = useAuth();
  const useEmojiReactions = currentUser?.use_emoji_reactions ?? true;
  const { registerNotifArrived, markRead, registerNotifViewer } = useStreamingContext();
  const { showError } = useToast();
  // キャッシュがあれば初回レンダーの時点から復元済みの内容を表示する（`useEffect`経由で
  // `setItems`すると、そのeffectが走るまでの最初の1回のレンダーが空一覧になり、その一瞬だけ
  // 実高さが縮んでwindow.scrollY（またはコンテナのscrollTop）がブラウザに強制的にクランプされ、
  // それが継続保存リスナーに拾われて正しいスクロール位置の記憶を0で上書きしてしまう）。
  const [loadingInitial, setLoadingInitial] = useState(() => cache === undefined);
  const itemsRef = useRef<NotificationItem[]>([]);

  const onError = useCallback((e: unknown) => showError(getErrorMessage(e)), [showError]);
  const [filter, setFilter] = useState<NotificationFilterState>(EMPTY_NOTIFICATION_FILTER);
  const query = toNotificationQuery(filter);
  const filterKey = JSON.stringify(query);
  const filterActive = query.includeTypes !== undefined || query.userIds !== undefined;
  // 以降のコールバックが最新の絞り込み条件を参照できるようにする（条件変更のたびに
  // コールバックを作り直して購読し直さないため）。
  const queryRef = useRef(query);
  queryRef.current = query;
  const fetchPage = useCallback(
    (untilId: string) => api.notifications.list({ limit: PAGE_SIZE, untilId, markAsRead: false, ...queryRef.current }),
    []
  );
  const { items, setItems, hasMore, setHasMore, loadingMore, loadMore } = useCursorPagination<NotificationItem>(
    fetchPage,
    (n) => n.id,
    PAGE_SIZE,
    onError,
    cache,
    filterKey
  );
  itemsRef.current = items;
  // 重ねるまとまりの境。一覧を取り直したときだけ、その時点の一番古い通知にする（新着の
  // 差し込み・古い通知の追加読み込みでは変えない。lib/notificationGroups.ts）。
  const [groupBaseId, setGroupBaseId] = useState<string | undefined>(
    () => cache?.groupBaseId ?? cache?.items[cache.items.length - 1]?.id
  );
  const replaceItems = useCallback(
    (rows: NotificationItem[]) => {
      setItems(rows);
      setGroupBaseId(rows[rows.length - 1]?.id);
    },
    [setItems]
  );
  // 空の一覧に新着が入ったときは、その時点の一番古い通知を境にする。
  useEffect(() => {
    if (groupBaseId === undefined && items.length > 0) setGroupBaseId(items[items.length - 1].id);
  }, [groupBaseId, items]);
  const groups = useMemo(() => groupNotifications(items, groupBaseId), [items, groupBaseId]);
  const sentinelRef = useInfiniteScrollSentinel<HTMLLIElement>(loadMore, hasMore);
  const listRef = useRef<HTMLUListElement>(null);

  // まとまりごとの状態（NotificationsPanelCache.decks）。キャッシュから戻したときは同じオブジェクトを
  // 使い続けて書き換える。絞り込み中のまとまりは絞り込み無しの一覧とは別なので、別に持って捨てる。
  const decksRef = useRef<Record<string, NotificationDeckState>>(cache?.decks ?? {});
  const filteredDecksRef = useRef<Record<string, NotificationDeckState>>({});
  const filterActiveRef = useRef(filterActive);
  filterActiveRef.current = filterActive;
  const deckState = useCallback((key: string): NotificationDeckState => {
    const decks = filterActiveRef.current ? filteredDecksRef.current : decksRef.current;
    decks[key] ??= {};
    return decks[key];
  }, []);

  /** 今のスクロール位置（一番上に見えている通知）。 */
  const captureScroll = useCallback(() => {
    const el = getScrollContainer?.() ?? null;
    return captureTimelineScroll(listRef.current, 0, el);
  }, [getScrollContainer]);
  // 覚えている位置。戻ってきたときはこれへ合わせる。
  const scrollRef = useRef(scroll);
  // 位置を合わせた直後のスクロール量。この値のままのスクロールイベントは自分で動かしたものなので、
  // ユーザーの位置として読まず、覚えている通知へ合わせ直す（HomePage の pinnedScrollY と同じ）。
  const pinnedTopRef = useRef<number | null>(null);
  const currentScrollTop = useCallback(() => {
    const el = getScrollContainer?.() ?? null;
    return el ? el.scrollTop : window.scrollY;
  }, [getScrollContainer]);
  const pinScroll = useCallback(() => {
    if (!scrollRef.current) return;
    applyTimelineScroll(listRef.current, scrollRef.current, getScrollContainer?.() ?? null);
    pinnedTopRef.current = currentScrollTop();
  }, [getScrollContainer, currentScrollTop]);
  // 合わせた位置から動いていなければ、もう一度合わせる（上に新着が入って高さが変わったとき。
  // ブラウザのスクロールアンカーが働かない環境向け）。
  const repinScroll = useCallback(() => {
    const pinnedTop = pinnedTopRef.current;
    if (pinnedTop !== null && Math.abs(currentScrollTop() - pinnedTop) < 1) pinScroll();
  }, [currentScrollTop, pinScroll]);

  // 遷移（ノート詳細・プロフィールへのnavigate）でこのコンポーネントがアンマウントされる際、
  // DOMが取り除かれている最中（cleanupでイベントリスナーが外れきるまでの一瞬）に実高さが
  // 大きく縮み、window.scrollY（またはコンテナのscrollTop）がブラウザに強制的に0へ
  // クランプされることがある。一覧が短い間は縮む処理が一瞬で終わり気づかないが、無限スクロールで
  // 深く読み込むほど（実高さが大きいほど）縮小に時間がかかり、その間に発生した'scroll'イベントを
  // 下記の継続保存リスナーが拾って正しい記憶を0で上書きしてしまう。
  // クリックした瞬間に同期的に現在値を確定・凍結し、以降の（クランプ由来の）上書きを止める
  // （HomePageの`navigatingAway`/`onBeforeNavigate`と同じ対策）。
  const navigatingAwayRef = useRef(false);
  const freezeScroll = useCallback(() => {
    navigatingAwayRef.current = true;
    onScrollChange?.(captureScroll());
  }, [onScrollChange, captureScroll]);

  // 未読だった（今回の取得で初めて表示した）通知。少しの間だけ背景色で目立たせる。
  // 既読にした取得の結果だけを対象にする（絞り込み中は既読にしないので、次に開いても未読のまま
  // また目立ってしまう）。キャッシュから戻した一覧は前に表示済みなので目立たせない。
  const [freshIds, setFreshIds] = useState<ReadonlySet<string>>(() => new Set());
  const freshTimersRef = useRef<number[]>([]);
  const highlightFresh = useCallback((rows: NotificationItem[]) => {
    const ids = rows.filter((r) => r.isRead === false).map((r) => r.id);
    if (ids.length === 0) return;
    setFreshIds((current) => new Set([...current, ...ids]));
    const timer = window.setTimeout(() => {
      freshTimersRef.current = freshTimersRef.current.filter((t) => t !== timer);
      setFreshIds((current) => {
        const next = new Set(current);
        ids.forEach((id) => next.delete(id));
        return next;
      });
    }, FRESH_HIGHLIGHT_MS);
    freshTimersRef.current.push(timer);
  }, []);
  useEffect(() => () => freshTimersRef.current.forEach((t) => window.clearTimeout(t)), []);

  // 既読にしながら表示している間は、未読バッジを出さない（StreamingContext.registerNotifViewer）。
  useEffect(() => {
    if (filterActive) return;
    return registerNotifViewer();
  }, [filterActive, registerNotifViewer]);

  // 指定ID（無ければ先頭ページ相当）より新しい通知を取得して先頭へ差し込み、既読マークする。
  // 初回マウント時（キャッシュ復元時の補完）とWS新着シグナル受信時の両方で共有する。
  // 絞り込み中は、一覧に出ていない通知まで既読にしてしまわないよう既読化しない。
  const mergeSince = useCallback(
    (sinceId: string | undefined) => {
      const markAsRead = queryRef.current.includeTypes === undefined && queryRef.current.userIds === undefined;
      return api.notifications.list({ limit: PAGE_SIZE, sinceId, markAsRead, ...queryRef.current }).then((rows) => {
        if (rows.length === 0) return;
        setItems((prev) => {
          const seen = new Set(prev.map((p) => p.id));
          const fresh = rows.filter((r) => !seen.has(r.id));
          return fresh.length > 0 ? [...fresh, ...prev] : prev;
        });
        if (markAsRead) {
          markRead();
          highlightFresh(rows);
        }
      });
    },
    [setItems, markRead, highlightFresh]
  );

  useEffect(() => {
    let cancelled = false;
    // items/hasMore/loadingInitialは既に初期値としてcacheから復元済み（上記）なので、ここでは
    // 離脱中に届いた新着だけをsinceIdで補って先頭へ差し込み、既読マークする
    // （HomePageの`mergeHeadIntoTimeline`と同じ、真のfire-and-forget）。
    if (cache) {
      mergeSince(cache.items[0]?.id)
        .then(() => requestAnimationFrame(() => requestAnimationFrame(() => !cancelled && repinScroll())))
        .catch((e) => !cancelled && onError(e));
      return () => {
        cancelled = true;
      };
    }
    api
      .notifications.list({ limit: PAGE_SIZE, markAsRead: true })
      .then((rows) => {
        if (cancelled) return;
        replaceItems(rows);
        setHasMore(rows.length >= PAGE_SIZE);
        markRead();
        highlightFresh(rows);
      })
      .catch((e) => !cancelled && onError(e))
      .finally(() => !cancelled && setLoadingInitial(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 絞り込み条件が変わったら、その条件で先頭ページから取得し直す（初回は上のeffectが取得する）。
  const loadedKeyRef = useRef(filterKey);
  useEffect(() => {
    if (loadedKeyRef.current === filterKey) return;
    loadedKeyRef.current = filterKey;
    filteredDecksRef.current = {};
    let cancelled = false;
    setLoadingInitial(true);
    api.notifications
      .list({ limit: PAGE_SIZE, markAsRead: !filterActive, ...queryRef.current })
      .then((rows) => {
        if (cancelled) return;
        replaceItems(rows);
        setHasMore(rows.length >= PAGE_SIZE);
        if (!filterActive) {
          markRead();
          highlightFresh(rows);
        }
      })
      .catch((e) => !cancelled && onError(e))
      .finally(() => !cancelled && setLoadingInitial(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  useEffect(
    () => registerNotifArrived(() => mergeSince(itemsRef.current[0]?.id).catch(onError)),
    [registerNotifArrived, mergeSince, onError]
  );

  // 一覧・hasMoreが変わるたびに呼び出し元のキャッシュへ書き戻す（HomePageのタイムラインと同じ方式）。
  // 初回読み込み中は書き込まない: React 18 StrictMode（開発時）はmount直後に同一レンダーの
  // effectを2回連続実行するため、まだ反映されていない「更新前の古いitems（空配列）」を
  // このeffectが読んでしまい、直前に復元/フェッチ中の正しいキャッシュを空データで
  // 上書きしてしまうのを避ける（HomePage側の同種コメント参照）。
  useEffect(() => {
    // 絞り込み中の一覧は書き戻さない（キャッシュは絞り込み無しの一覧として復元されるため）。
    if (loadingInitial || !onCacheChange || filterActive) return;
    onCacheChange({ items, hasMore, groupBaseId, decks: decksRef.current });
  }, [items, hasMore, groupBaseId, loadingInitial, onCacheChange, filterActive]);

  // スクロール位置は継続的に（都度）呼び出し元へ書き戻す（HomePageのタイムラインと同じ方式）。
  // 画素値ではなく一番上に見えている通知で覚えるので、離れている間に新着が上に入っても、
  // 重ねた通知が広がって高さが変わっても、同じ通知を同じ位置へ戻せる。
  // navigatingAwayRefが立った後（上記freezeScroll参照）は、DOM除去中のクランプ由来の
  // 'scroll'イベントを拾って正しい記憶を上書きしないよう保存を止める。復元前（一覧の描画直後）も
  // 読まない（復元前の位置で覚えていた位置を上書きしてしまう）。
  const scrollRestoredRef = useRef(false);
  useEffect(() => {
    if (!onScrollChange) return;
    const el = getScrollContainer?.() ?? null;
    const onScroll = () => {
      if (navigatingAwayRef.current || !scrollRestoredRef.current) return;
      if (!listRef.current?.isConnected) return;
      const pinnedTop = pinnedTopRef.current;
      if (pinnedTop !== null && Math.abs(currentScrollTop() - pinnedTop) < 1) {
        if (scrollRef.current?.kind === "anchor") pinScroll();
        return;
      }
      pinnedTopRef.current = null;
      const next = captureScroll();
      scrollRef.current = next;
      onScrollChange(next);
    };
    const target = el ?? window;
    target.addEventListener("scroll", onScroll, { passive: true });
    return () => target.removeEventListener("scroll", onScroll);
  }, [onScrollChange, getScrollContainer, captureScroll, currentScrollTop, pinScroll]);

  // 初回読み込みが終わり一覧がDOMへ反映された後に、一度だけスクロール位置を復元する
  // （絞り込みによる再読み込みでは復元しない）。
  useEffect(() => {
    if (loadingInitial || scrollRestoredRef.current) return;
    if (scroll === undefined) {
      scrollRestoredRef.current = true;
      return;
    }
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        pinScroll();
        scrollRestoredRef.current = true;
      })
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingInitial]);

  const filterBar = <NotificationFilterBar filter={filter} onChange={setFilter} />;

  if (loadingInitial) {
    return (
      <>
        {filterBar}
        <div className={panel.placeholder}>{t("common:loading")}</div>
      </>
    );
  }

  if (items.length === 0) {
    return (
      <>
        {filterBar}
        <div className={panel.placeholder}>
          <UiIcon emoji="🔔" className={panel.placeholderIcon} />
          {filterActive ? (
            t("notifications:notificationsPanel.noMatchingNotifications")
          ) : (
            <>
              {t("notifications:notificationsPanel.noNotifications")}
              <br />
              {t("notifications:notificationsPanel.noNotificationsDetail")}
            </>
          )}
        </div>
      </>
    );
  }

  // 通知1件のカード。重ねるまとまりの中では li ではなく div にする（li は CardStack が持つ）。
  const renderCard = (n: NotificationItem, stacked: boolean) => {
    const { icon, iconUrl, i18nKey, who, whoEmojis, whoAcct, newHandle } = describeNotification(n, useEmojiReactions);
    const noteId = resolveTargetNoteId(n);
    const clickNoteId = resolveClickTargetNoteId(n);
    const userLink = n.user?.username ? (
      <UserLinkTag
        target={toRelationshipTarget(n.user)}
        to={profilePath(n.user.username, n.user.host ?? undefined)}
        className={styles.userLink}
        title={whoAcct}
      />
    ) : (
      <span />
    );
    const newUserLink = n.relatedUser?.username ? (
      <UserLinkTag
        target={toRelationshipTarget(n.relatedUser)}
        to={profilePath(n.relatedUser.username, n.relatedUser.host ?? undefined)}
        className={styles.userLink}
        title={notificationUserAcct(n.relatedUser)}
      />
    ) : (
      <span />
    );
    const emojiName = <DisplayName name={who} fallback="" emojis={whoEmojis} />;
    const avatar = <Avatar url={n.user?.avatarUrl} name={who} size={20} />;
    const message = (
      <Trans
        i18n={i18n}
        i18nKey={i18nKey}
        values={{ newHandle }}
        components={{ userLink, emojiName, newUserLink }}
      />
    );
    const twoLine = TWO_LINE_TYPES.has(n.type);
    const summary = twoLine ? targetSummary(n) : "";
    const content = (
      <>
        {iconUrl ? (
          <RemoteImg className={styles.iconImg} src={iconUrl} alt={icon} title={icon} loading="lazy" data-emoji="" />
        ) : (
          n.type === "reaction" && useEmojiReactions ? (
            <TwemojiEmoji emoji={icon} className={styles.icon} />
          ) : (
            <UiIcon emoji={icon} className={styles.icon} />
          )
        )}
        {n.user?.username ? (
          <UserHoverArea
            target={{ username: n.user.username, domain: n.user.host ?? undefined }}
            isSelf={isSelfUser(currentUser, n.user)}
          >
            <UserContextMenu target={toRelationshipTarget(n.user)}>
              <Link
                to={profilePath(n.user.username, n.user.host ?? undefined)}
                className={styles.avatarLink}
                onClick={(e) => e.stopPropagation()}
              >
                {avatar}
              </Link>
            </UserContextMenu>
          </UserHoverArea>
        ) : (
          avatar
        )}
        {twoLine ? (
          <span className={styles.lines}>
            <span className={styles.singleLine}>{message}</span>
            {summary && (
              <span className={`${styles.singleLine} ${styles.targetLine}`}>
                <EmojiText text={summary} emojis={targetNote(n)?.emojis} />
              </span>
            )}
          </span>
        ) : (
          <span className={styles.text}>{message}</span>
        )}
      </>
    );
    const Tag = stacked ? "div" : "li";
    const className = [
      styles.item,
      clickNoteId && styles.clickable,
      stacked && styles.stackItem,
      freshIds.has(n.id) && styles.fresh,
    ]
      .filter(Boolean)
      .join(" ");
    return (
      <Tag
        key={n.id}
        className={className}
        data-notification-id={n.id}
        data-timeline-id={stacked ? undefined : n.id}
        onClick={
          clickNoteId
            ? () => {
                freezeScroll();
                navigate(`/notes/${clickNoteId}`);
              }
            : undefined
        }
      >
        {noteId ? (
          <NoteHoverPreview noteId={noteId} className={styles.previewWrap} side="left">
            {content}
          </NoteHoverPreview>
        ) : (
          content
        )}
      </Tag>
    );
  };

  return (
    <>
    {filterBar}
    <ul
      ref={listRef}
      className={styles.list}
      onClickCapture={(event) => {
        // AppShellのonBeforeNavigateと同じ判定（`a[href]`クリック）。上記`<li>`のonClick
        // （ノート詳細へのnavigate、`a`タグではない）は個別にfreezeScrollを呼んでいるため、
        // ここでは二重発火してもnavigatingAwayRef.currentがtrueになるだけで無害。
        if ((event.target as Element).closest("a[href]")) freezeScroll();
      }}
    >
      {groups.map((g) =>
        g.items.length === 1 ? (
          renderCard(g.items[0], false)
        ) : (
          <CardStack
            key={g.items[g.items.length - 1].id}
            as="li"
            className={styles.stack}
            keys={g.items.map((n) => n.id)}
            containerProps={{ "data-timeline-id": g.items[0].id }}
            renderCard={(i) => renderCard(g.items[i], true)}
            spreadOffsetPx={DECK_SPREAD_OFFSET_PX}
            pinnedClassName={styles.stackPinned}
            initialFront={deckState(g.items[g.items.length - 1].id).front}
            onFrontChange={(id) => {
              deckState(g.items[g.items.length - 1].id).front = id;
            }}
            initialPinned={deckState(g.items[g.items.length - 1].id).pinned}
            onPinnedChange={(pinned) => {
              deckState(g.items[g.items.length - 1].id).pinned = pinned;
            }}
          />
        )
      )}
      {hasMore && (
        <li ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </li>
      )}
    </ul>
    </>
  );
}
