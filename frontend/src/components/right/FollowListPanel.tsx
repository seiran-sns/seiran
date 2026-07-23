import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { api, FollowListItem } from "../../api/client";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import { profilePath } from "../../lib/format";
import Avatar from "../note/Avatar";
import panel from "../common/Panel.module.css";
import styles from "./FollowListPanel.module.css";

const PAGE_SIZE = 30;

interface FollowListPanelProps {
  actorId: string;
  kind: "following" | "followers";
  onError: (err: unknown) => void;
}

/** プロフィール右ペインの「フォロー中」「フォロワー」タブの中身（#56）。無限スクロール。 */
export default function FollowListPanel({ actorId, kind, onError }: FollowListPanelProps) {
  const { t } = useTranslation();
  const [initialLoading, setInitialLoading] = useState(true);
  const fetchFn = kind === "following" ? api.users.following : api.users.followers;

  const fetchPage = useCallback((untilId: string) => fetchFn(actorId, { limit: PAGE_SIZE, until_id: untilId }), [actorId, fetchFn]);
  const { items, setItems, hasMore, setHasMore, loadingMore, loadMore } = useCursorPagination<FollowListItem>(
    fetchPage,
    (item) => item.follow_id,
    PAGE_SIZE,
    onError
  );

  useEffect(() => {
    let cancelled = false;
    setInitialLoading(true);
    fetchFn(actorId, { limit: PAGE_SIZE })
      .then((rows) => {
        if (cancelled) return;
        setItems(rows);
        setHasMore(rows.length >= PAGE_SIZE);
      })
      .catch((e) => !cancelled && onError(e))
      .finally(() => !cancelled && setInitialLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [actorId, kind]);

  const sentinelRef = useInfiniteScrollSentinel<HTMLDivElement>(loadMore, hasMore);

  if (initialLoading) return <p className={panel.message}>{t("common:loading")}</p>;

  const emptyMessage =
    kind === "following" ? t("profile:profilePage.followList.noFollowing") : t("profile:profilePage.followList.noFollowers");
  if (items.length === 0) return <p className={panel.message}>{emptyMessage}</p>;

  return (
    <div className={styles.list}>
      {items.map((item) => (
        <Link key={item.follow_id} to={profilePath(item.username, item.domain)} className={styles.row}>
          <Avatar url={item.avatar_url} name={item.display_name || item.username} size={40} />
          <div className={styles.names}>
            <span className={styles.displayName}>{item.display_name || item.username}</span>
            <span className={styles.acct}>
              @{item.username}
              {item.domain && item.domain !== window.location.hostname && `@${item.domain}`}
            </span>
          </div>
        </Link>
      ))}
      {hasMore && (
        <div ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </div>
      )}
    </div>
  );
}
