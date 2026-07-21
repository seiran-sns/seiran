import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import i18n from "../../i18n";
import { api, getErrorMessage, NotificationItem } from "../../api/client";
import { useStreamingContext } from "../../contexts/StreamingContext";
import { useToast } from "../../contexts/ToastContext";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import panel from "../common/Panel.module.css";
import styles from "./NotificationsPanel.module.css";

const PAGE_SIZE = 20;

/** 通知1件を人間可読な文言に整形する。`iconUrl` があれば絵文字は画像（カスタム絵文字）。 */
function describe(n: NotificationItem): { icon: string; iconUrl?: string; text: string } {
  const who = n.user?.name || n.user?.username || i18n.t("notifications:notificationsPanel.unknownUser");
  const handle = n.user?.username && n.user?.host ? `@${n.user.username}@${n.user.host}` : "";
  const label = handle ? `${who}（${handle}）` : who;
  switch (n.type) {
    case "reaction":
      return {
        icon: n.reaction || "⭐",
        iconUrl: n.reaction ? n.note?.reactionEmojis?.[n.reaction] : undefined,
        text: i18n.t("notifications:notificationsPanel.reactionText", { label }),
      };
    case "follow":
      return { icon: "➕", text: i18n.t("notifications:notificationsPanel.followText", { label }) };
    case "followRequestAccepted":
      return { icon: "🤝", text: i18n.t("notifications:notificationsPanel.followAcceptedText", { label }) };
    case "mention":
      return { icon: "📣", text: i18n.t("notifications:notificationsPanel.mentionText", { label }) };
    default:
      return { icon: "🔔", text: i18n.t("notifications:notificationsPanel.genericText", { label }) };
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
        <span className={panel.placeholderIcon}>🔔</span>
        {t("notifications:notificationsPanel.noNotifications")}
        <br />
        {t("notifications:notificationsPanel.noNotificationsDetail")}
      </div>
    );
  }

  return (
    <ul className={styles.list}>
      {items.map((n) => {
        const { icon, iconUrl, text } = describe(n);
        const noteId = n.type === "mention" ? n.note?.id : undefined;
        return (
          <li key={n.id} className={styles.item}>
            {iconUrl ? (
              <img className={styles.iconImg} src={iconUrl} alt={icon} title={icon} loading="lazy" />
            ) : (
              <span className={styles.icon}>{icon}</span>
            )}
            {noteId ? (
              <Link to={`/notes/${noteId}`} className={styles.text}>
                {text}
              </Link>
            ) : (
              <span className={styles.text}>{text}</span>
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
