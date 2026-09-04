import { useCallback, useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import i18n from "../../i18n";
import { api, getErrorMessage, NotificationItem } from "../../api/client";
import type { NotificationUser } from "../../api/types";
import { useAuth } from "../../contexts/AuthContext";
import { useStreamingContext } from "../../contexts/StreamingContext";
import { useToast } from "../../contexts/ToastContext";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import { UserRelationshipTarget } from "../../hooks/useUserRelationshipMenu";
import { profilePath } from "../../lib/format";
import { parseReactionContent } from "../../lib/customEmojis";
import { mediaUrl } from "../../utils/mediaProxy";
import panel from "../common/Panel.module.css";
import Avatar from "../note/Avatar";
import EmojiText from "../note/EmojiText";
import NoteHoverPreview from "../note/NoteHoverPreview";
import UserContextMenu from "../note/UserContextMenu";
import UserHoverArea from "../note/UserHoverArea";
import UserLinkTag from "../note/UserLinkTag";
import styles from "./NotificationsPanel.module.css";
import TwemojiEmoji from "../common/TwemojiEmoji";

/** 通知ユーザーを対ユーザー操作メニュー用の`UserRelationshipTarget`に変換する。 */
function toRelationshipTarget(u: NotificationUser): UserRelationshipTarget {
  return {
    username: u.username,
    domain: u.host ?? undefined,
    actorId: u.id,
    reportLabel: `@${u.username}${u.host ? `@${u.host}` : ""}`,
  };
}

/** 通知に出てくるユーザーが閲覧者自身かどうか。 */
function isSelfUser(currentUser: { username: string } | null, u?: NotificationUser): boolean {
  return !!currentUser && !!u && currentUser.username === u.username && (!u.host || u.host === window.location.hostname);
}

/** Misskey本家仕様のコロンなしshortcodeキー（`user.emojis`）を、`EmojiText` が期待する
 * `:shortcode:` 形式のキーへ変換する（#186）。 */
function colonizeEmojiKeys(emojis?: Record<string, string>): Record<string, string> | undefined {
  if (!emojis) return undefined;
  return Object.fromEntries(Object.entries(emojis).map(([code, url]) => [`:${code}:`, url]));
}

const PAGE_SIZE = 20;

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
 * （`note.id`）へ飛ばす（リポストという行為そのものの投稿ページを開くため）。 */
export function resolveClickTargetNoteId(n: NotificationItem): string | undefined {
  if (!NOTE_LINKED_TYPES.has(n.type)) return undefined;
  return n.note?.id;
}

/** 通知1件を人間可読な文言に整形する。`iconUrl` があれば絵文字は画像（カスタム絵文字）。
 * `who`（表示名部分）は呼び出し側で `EmojiText` を通す前提でプレーンテキストのまま返す。 */
export function describeNotification(n: NotificationItem): {
  icon: string;
  iconUrl?: string;
  i18nKey: string;
  who: string;
  whoEmojis?: Record<string, string>;
  handleSuffix: string;
  newHandle?: string;
} {
  const who = n.user?.name || n.user?.username || i18n.t("notifications:notificationsPanel.unknownUser");
  const handle = n.user?.username && n.user?.host ? `@${n.user.username}@${n.user.host}` : "";
  const handleSuffix = handle ? `（${handle}）` : "";
  const whoEmojis = n.user?.emojis;
  const newHandle =
    n.relatedUser?.username && n.relatedUser?.host
      ? `@${n.relatedUser.username}@${n.relatedUser.host}`
      : n.relatedUser?.name || n.relatedUser?.username;
  switch (n.type) {
    case "reaction": {
      // `reactionEmojis` のキーは Misskey 本家仕様に合わせコロンなし shortcode
      // （ローカルは `shortcode@.`、リモートは `shortcode@host`。バックエンド側 `convert.rs`）。
      // `reaction` は `:shortcode:`/`:shortcode@host:` 形式なので分解してから引く。
      const parsedReaction = n.reaction ? parseReactionContent(n.reaction) : null;
      const emojiKey = parsedReaction
        ? parsedReaction.shortcode + (parsedReaction.host ? `@${parsedReaction.host}` : "")
        : undefined;
      return {
        icon: n.reaction || "⭐",
        iconUrl: emojiKey ? n.note?.reactionEmojis?.[emojiKey] : undefined,
        i18nKey: "notifications:notificationsPanel.reactionText",
        who,
        whoEmojis,
        handleSuffix,
      };
    }
    case "follow":
      return { icon: "➕", i18nKey: "notifications:notificationsPanel.followText", who, whoEmojis, handleSuffix };
    // バックエンド`to_misskey_notification_type`が内部種別`followRequest`をMisskey本家の
    // `receiveFollowRequest`へ変換して返すため、ここでもその値を見る（`repost`→`renote`と同様）。
    case "receiveFollowRequest":
      return { icon: "🙋", i18nKey: "notifications:notificationsPanel.followRequestText", who, whoEmojis, handleSuffix };
    case "followRequestAccepted":
      return { icon: "🤝", i18nKey: "notifications:notificationsPanel.followAcceptedText", who, whoEmojis, handleSuffix };
    case "mention":
      return { icon: "📣", i18nKey: "notifications:notificationsPanel.mentionText", who, whoEmojis, handleSuffix };
    case "reply":
      return { icon: "💬", i18nKey: "notifications:notificationsPanel.replyText", who, whoEmojis, handleSuffix };
    case "renote":
      return { icon: "🔁", i18nKey: "notifications:notificationsPanel.repostText", who, whoEmojis, handleSuffix };
    case "quote":
      return { icon: "❝", i18nKey: "notifications:notificationsPanel.quoteText", who, whoEmojis, handleSuffix };
    case "moveRefollowed":
      return {
        icon: "🚚",
        i18nKey: "notifications:notificationsPanel.moveRefollowedText",
        who,
        whoEmojis,
        handleSuffix,
        newHandle,
      };
    case "moveAlreadyFollowing":
      return {
        icon: "🚚",
        i18nKey: "notifications:notificationsPanel.moveAlreadyFollowingText",
        who,
        whoEmojis,
        handleSuffix,
        newHandle,
      };
    default:
      return { icon: "🔔", i18nKey: "notifications:notificationsPanel.genericText", who, whoEmojis, handleSuffix };
  }
}

interface NotificationsPanelCache {
  items: NotificationItem[];
  hasMore: boolean;
}

interface NotificationsPanelProps {
  /** 復元すべきスクロール位置。省略時はスクロール位置の保存・復元を行わない。 */
  scrollY?: number;
  /** スクロールのたびに現在位置を書き戻すコールバック。 */
  onScrollYChange?: (y: number) => void;
  /** スクロールを監視・復元する要素を返す関数。省略時は`window`
   *（通知一覧画面のように中央ペインでwindowスクロールする場合）。
   * Home/Search画面の右ペインのようにコンテナ自身が`overflow-y: auto`で
   * 独立スクロールする場合はそのコンテナを返す関数を渡す。 */
  getScrollContainer?: () => HTMLElement | null;
  /** 前回表示時の一覧（追加読み込み分含む）。渡された場合は再フェッチせずこれを初期表示に使い、
   * 離脱中に届いた新着だけを補って差し込む。これが無いと`scrollY`だけ復元しても、
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
  scrollY,
  onScrollYChange,
  getScrollContainer,
  cache,
  onCacheChange,
}: NotificationsPanelProps = {}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { user: currentUser } = useAuth();
  const { registerNotifArrived, markRead } = useStreamingContext();
  const { showError } = useToast();
  // キャッシュがあれば初回レンダーの時点から復元済みの内容を表示する（`useEffect`経由で
  // `setItems`すると、そのeffectが走るまでの最初の1回のレンダーが空一覧になり、その一瞬だけ
  // 実高さが縮んでwindow.scrollY（またはコンテナのscrollTop）がブラウザに強制的にクランプされ、
  // それが継続保存リスナーに拾われて正しいスクロール位置の記憶を0で上書きしてしまう不具合があった）。
  const [loadingInitial, setLoadingInitial] = useState(() => cache === undefined);
  const itemsRef = useRef<NotificationItem[]>([]);

  const onError = useCallback((e: unknown) => showError(getErrorMessage(e)), [showError]);
  const fetchPage = useCallback(
    (untilId: string) => api.notifications.list({ limit: PAGE_SIZE, untilId, markAsRead: false }),
    []
  );
  const { items, setItems, hasMore, setHasMore, loadingMore, loadMore } = useCursorPagination<NotificationItem>(
    fetchPage,
    (n) => n.id,
    PAGE_SIZE,
    onError,
    cache
  );
  itemsRef.current = items;
  const sentinelRef = useInfiniteScrollSentinel<HTMLLIElement>(loadMore, hasMore);

  // 遷移（ノート詳細・プロフィールへのnavigate）でこのコンポーネントがアンマウントされる際、
  // DOMが取り除かれている最中（cleanupでイベントリスナーが外れきるまでの一瞬）に実高さが
  // 大きく縮み、window.scrollY（またはコンテナのscrollTop）がブラウザに強制的に0へ
  // クランプされることがある。一覧が短い間は縮む処理が一瞬で終わり気づかないが、無限スクロールで
  // 深く読み込むほど（実高さが大きいほど）縮小に時間がかかり、その間に発生した'scroll'イベントを
  // 下記の継続保存リスナーが拾って正しい記憶を0で上書きしてしまう不具合があった（実機で確認）。
  // クリックした瞬間に同期的に現在値を確定・凍結し、以降の（クランプ由来の）上書きを止める
  // （HomePageの`navigatingAway`/`onBeforeNavigate`と同じ対策）。
  const navigatingAwayRef = useRef(false);
  const freezeScroll = useCallback(() => {
    navigatingAwayRef.current = true;
    if (!onScrollYChange) return;
    const el = getScrollContainer?.() ?? null;
    onScrollYChange(el ? el.scrollTop : window.scrollY);
  }, [onScrollYChange, getScrollContainer]);

  // 指定ID（無ければ先頭ページ相当）より新しい通知を取得して先頭へ差し込み、既読マークする。
  // 初回マウント時（キャッシュ復元時の補完）とWS新着シグナル受信時の両方で共有する。
  const mergeSince = useCallback(
    (sinceId: string | undefined) =>
      api.notifications.list({ limit: PAGE_SIZE, sinceId, markAsRead: true }).then((rows) => {
        if (rows.length === 0) return;
        setItems((prev) => {
          const seen = new Set(prev.map((p) => p.id));
          const fresh = rows.filter((r) => !seen.has(r.id));
          return fresh.length > 0 ? [...fresh, ...prev] : prev;
        });
        markRead();
      }),
    [setItems, markRead]
  );

  useEffect(() => {
    let cancelled = false;
    // items/hasMore/loadingInitialは既に初期値としてcacheから復元済み（上記）なので、ここでは
    // 離脱中に届いた新着だけをsinceIdで補って先頭へ差し込み、既読マークする
    // （HomePageの`mergeHeadIntoTimeline`と同じ、真のfire-and-forget）。
    if (cache) {
      mergeSince(cache.items[0]?.id).catch((e) => !cancelled && onError(e));
      return () => {
        cancelled = true;
      };
    }
    api
      .notifications.list({ limit: PAGE_SIZE, markAsRead: true })
      .then((rows) => {
        if (cancelled) return;
        setItems(rows);
        setHasMore(rows.length >= PAGE_SIZE);
        markRead();
      })
      .catch((e) => !cancelled && onError(e))
      .finally(() => !cancelled && setLoadingInitial(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(
    () => registerNotifArrived(() => mergeSince(itemsRef.current[0]?.id).catch(onError)),
    [registerNotifArrived, mergeSince, onError]
  );

  // 一覧・hasMoreが変わるたびに呼び出し元のキャッシュへ書き戻す（HomePageのタイムラインと同じ方式）。
  // 初回読み込み中は書き込まない: React 18 StrictMode（開発時）はmount直後に同一レンダーの
  // effectを2回連続実行するため、まだ反映されていない「更新前の古いitems（空配列）」を
  // このeffectが読んでしまい、直前に復元/フェッチ中の正しいキャッシュを空データで
  // 上書きしてしまう不具合を避ける（HomePage側の同種コメント参照）。
  useEffect(() => {
    if (loadingInitial || !onCacheChange) return;
    onCacheChange({ items, hasMore });
  }, [items, hasMore, loadingInitial, onCacheChange]);

  // スクロール位置は継続的に（都度）呼び出し元へ書き戻す（HomePageのタイムラインと同じ方式）。
  // navigatingAwayRefが立った後（上記freezeScroll参照）は、DOM除去中のクランプ由来の
  // 'scroll'イベントを拾って正しい記憶を上書きしないよう保存を止める。
  useEffect(() => {
    if (!onScrollYChange) return;
    const el = getScrollContainer?.() ?? null;
    const onScroll = () => {
      if (navigatingAwayRef.current) return;
      onScrollYChange(el ? el.scrollTop : window.scrollY);
    };
    const target = el ?? window;
    target.addEventListener("scroll", onScroll, { passive: true });
    return () => target.removeEventListener("scroll", onScroll);
  }, [onScrollYChange, getScrollContainer]);

  // 初回読み込みが終わり一覧がDOMへ反映された後に、一度だけスクロール位置を復元する。
  useEffect(() => {
    if (loadingInitial || scrollY === undefined) return;
    const el = getScrollContainer?.() ?? null;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (el) el.scrollTop = scrollY;
        else window.scrollTo(0, scrollY);
      })
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingInitial]);

  if (loadingInitial) {
    return <div className={panel.placeholder}>{t("common:loading")}</div>;
  }

  if (items.length === 0) {
    return (
      <div className={panel.placeholder}>
        <TwemojiEmoji emoji="🔔" className={panel.placeholderIcon} />
        {t("notifications:notificationsPanel.noNotifications")}
        <br />
        {t("notifications:notificationsPanel.noNotificationsDetail")}
      </div>
    );
  }

  return (
    <ul
      className={styles.list}
      onClickCapture={(event) => {
        // AppShellのonBeforeNavigateと同じ判定（`a[href]`クリック）。上記`<li>`のonClick
        // （ノート詳細へのnavigate、`a`タグではない）は個別にfreezeScrollを呼んでいるため、
        // ここでは二重発火してもnavigatingAwayRef.currentがtrueになるだけで無害。
        if ((event.target as Element).closest("a[href]")) freezeScroll();
      }}
    >
      {items.map((n) => {
        const { icon, iconUrl, i18nKey, who, whoEmojis, handleSuffix, newHandle } = describeNotification(n);
        const noteId = resolveTargetNoteId(n);
        const clickNoteId = resolveClickTargetNoteId(n);
        const userLink = n.user?.username ? (
          <UserLinkTag
            target={toRelationshipTarget(n.user)}
            to={profilePath(n.user.username, n.user.host ?? undefined)}
            className={styles.userLink}
          />
        ) : (
          <span />
        );
        const newUserLink = n.relatedUser?.username ? (
          <UserLinkTag
            target={toRelationshipTarget(n.relatedUser)}
            to={profilePath(n.relatedUser.username, n.relatedUser.host ?? undefined)}
            className={styles.userLink}
          />
        ) : (
          <span />
        );
        const emojiName = <EmojiText text={who} emojis={colonizeEmojiKeys(whoEmojis)} />;
        const avatar = <Avatar url={n.user?.avatarUrl} name={who} size={20} />;
        const content = (
          <>
            {iconUrl ? (
              <img className={styles.iconImg} src={mediaUrl(iconUrl)} alt={icon} title={icon} loading="lazy" />
            ) : (
              <TwemojiEmoji emoji={icon} className={styles.icon} />
            )}
            {n.user?.username ? (
              <UserHoverArea
                target={{ username: n.user.username, domain: n.user.host ?? undefined }}
                isSelf={isSelfUser(currentUser, n.user)}
              >
                <UserContextMenu target={toRelationshipTarget(n.user)}>
                  <span className={styles.avatarLink}>{avatar}</span>
                </UserContextMenu>
              </UserHoverArea>
            ) : (
              avatar
            )}
            <span className={styles.text}>
              <Trans
                i18n={i18n}
                i18nKey={i18nKey}
                values={{ handleSuffix, newHandle }}
                components={{ userLink, emojiName, newUserLink }}
              />
            </span>
          </>
        );
        return (
          <li
            key={n.id}
            className={clickNoteId ? `${styles.item} ${styles.clickable}` : styles.item}
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
          </li>
        );
      })}
      {hasMore && (
        <li ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </li>
      )}
    </ul>
  );
}
