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

/**
 * ホーム右ペイン タブ2: クイック通知（Doc5 §2.1）。
 * `POST /api/i/notifications`（Misskey API 互換, Doc3 §5.5）で永続化された通知履歴を
 * 新着順に読み込み、下端までスクロールすると `untilId` カーソルで過去分を追加取得する。
 * WS 経由のライブ通知（`registerNotifArrived`）は「新着があった」というシグナルにのみ使い、
 * 実データは常に REST から取得することで、一覧表示と整合したID体系を保つ。
 */
export default function NotificationsPanel() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { user: currentUser } = useAuth();
  const { registerNotifArrived, markRead } = useStreamingContext();
  const { showError } = useToast();
  const [loadingInitial, setLoadingInitial] = useState(true);
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
    onError
  );
  itemsRef.current = items;
  const sentinelRef = useInfiniteScrollSentinel<HTMLLIElement>(loadMore, hasMore);

  useEffect(() => {
    let cancelled = false;
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
    () =>
      registerNotifArrived(() => {
        const newestId = itemsRef.current[0]?.id;
        api.notifications.list({ limit: PAGE_SIZE, sinceId: newestId, markAsRead: true }).then((rows) => {
          if (rows.length === 0) return;
          setItems((prev) => {
            const seen = new Set(prev.map((p) => p.id));
            const fresh = rows.filter((r) => !seen.has(r.id));
            return fresh.length > 0 ? [...fresh, ...prev] : prev;
          });
          markRead();
        }).catch(onError);
      }),
    [registerNotifArrived, markRead, onError, setItems]
  );

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
    <ul className={styles.list}>
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
            onClick={clickNoteId ? () => navigate(`/notes/${clickNoteId}`) : undefined}
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
