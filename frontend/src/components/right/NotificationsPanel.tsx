import { useCallback, useEffect, useRef, useState } from "react";
import { api, NotificationItem } from "../../api/client";
import { useStreamingContext } from "../../contexts/StreamingContext";
import panel from "../common/Panel.module.css";
import styles from "./NotificationsPanel.module.css";

const PAGE_SIZE = 20;

/** 通知1件を人間可読な文言に整形する。`iconUrl` があれば絵文字は画像（カスタム絵文字）。 */
function describe(n: NotificationItem): { icon: string; iconUrl?: string; text: string } {
  const who = n.user?.name || n.user?.username || "だれか";
  const handle = n.user?.username && n.user?.host ? `@${n.user.username}@${n.user.host}` : "";
  const label = handle ? `${who}（${handle}）` : who;
  switch (n.type) {
    case "reaction":
      return {
        icon: n.reaction || "⭐",
        iconUrl: n.reaction ? n.note?.reactionEmojis?.[n.reaction] : undefined,
        text: `${label} がリアクションしました`,
      };
    case "follow":
      return { icon: "➕", text: `${label} にフォローされました` };
    case "followRequestAccepted":
      return { icon: "🤝", text: `${label} がフォローを承認しました` };
    default:
      return { icon: "🔔", text: `${label} から通知` };
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
  const { registerNotifArrived, markRead } = useStreamingContext();
  const [items, setItems] = useState<NotificationItem[]>([]);
  const [loadingInitial, setLoadingInitial] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(true);
  const itemsRef = useRef<NotificationItem[]>([]);
  const loadingMoreRef = useRef(false);
  const sentinelRef = useRef<HTMLLIElement | null>(null);
  itemsRef.current = items;

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
        });
      }),
    [registerNotifArrived, markRead]
  );

  const loadMore = useCallback(() => {
    if (loadingMoreRef.current || itemsRef.current.length === 0) return;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    const untilId = itemsRef.current[itemsRef.current.length - 1].id;
    api
      .notifications.list({ limit: PAGE_SIZE, untilId, markAsRead: false })
      .then((rows) => {
        setItems((prev) => {
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
  }, []);

  useEffect(() => {
    const el = sentinelRef.current;
    if (!el || !hasMore) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) loadMore();
      },
      { rootMargin: "200px" }
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [hasMore, loadMore, items.length]);

  if (loadingInitial) {
    return <div className={panel.placeholder}>読み込み中…</div>;
  }

  if (items.length === 0) {
    return (
      <div className={panel.placeholder}>
        <span className={panel.placeholderIcon}>🔔</span>
        新しい通知はありません。
        <br />
        リプライ・リアクション・フォローがここにリアルタイム表示されます。
      </div>
    );
  }

  return (
    <ul className={styles.list}>
      {items.map((n) => {
        const { icon, iconUrl, text } = describe(n);
        return (
          <li key={n.id} className={styles.item}>
            {iconUrl ? (
              <img className={styles.iconImg} src={iconUrl} alt={icon} title={icon} loading="lazy" />
            ) : (
              <span className={styles.icon}>{icon}</span>
            )}
            <span className={styles.text}>{text}</span>
          </li>
        );
      })}
      {hasMore && (
        <li ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? "読み込み中…" : ""}
        </li>
      )}
    </ul>
  );
}
