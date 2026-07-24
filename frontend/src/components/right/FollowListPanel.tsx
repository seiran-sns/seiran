import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { api, FollowListItem, RemoteFollowSummaryItem } from "../../api/client";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import { profilePath } from "../../lib/format";
import { getRemoteFollowSummary } from "../../lib/remoteFollowSummaryCache";
import Avatar from "../note/Avatar";
import panel from "../common/Panel.module.css";
import styles from "./FollowListPanel.module.css";

const PAGE_SIZE = 30;

interface FollowListPanelProps {
  actorId: string;
  kind: "following" | "followers";
  onError: (err: unknown) => void;
  /** リモートFediアクターのプロフィールの場合 true。ローカルDBが把握している範囲を
   * 超えて、相手サーバーへ直接 AP 経由で全件取得を試みる（#68）。 */
  isRemoteFedi?: boolean;
}

/** プロフィール右ペインの「フォロー中」「フォロワー」タブの中身（#56）。無限スクロール。 */
export default function FollowListPanel({ actorId, kind, onError, isRemoteFedi }: FollowListPanelProps) {
  const { t } = useTranslation();
  const [initialLoading, setInitialLoading] = useState(true);
  const fetchFn = kind === "following" ? api.users.following : api.users.followers;

  // リモートFediサーバーへの直接問い合わせによる全件取得（#68）。ローカルDBが把握して
  // いない関係（seiranが認知していないリモート同士のフォロー）を補完表示する。
  const [remoteExtra, setRemoteExtra] = useState<RemoteFollowSummaryItem[]>([]);
  const [remoteState, setRemoteState] = useState<"idle" | "loading" | "pending" | "done">("idle");

  useEffect(() => {
    if (!isRemoteFedi) {
      setRemoteExtra([]);
      setRemoteState("idle");
      return;
    }
    let cancelled = false;
    setRemoteState("loading");
    // プロフィール画面ロード時点で先読み済み（`prefetchRemoteFollowSummary`、#68）ならそれを
    // 再利用する。タブを開いた瞬間の待たされた感を無くすのが狙い。
    getRemoteFollowSummary(actorId, kind)
      .then((res) => {
        if (cancelled) return;
        setRemoteExtra(res.items);
        setRemoteState(res.pending ? "pending" : "done");
      })
      .catch(() => !cancelled && setRemoteState("idle"));
    return () => {
      cancelled = true;
    };
  }, [actorId, kind, isRemoteFedi]);

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

  // リモートで取得できた項目のうち、ローカルDBが既に把握している（=上のリストに出ている）
  // アクターは重複表示しない。マイケル指摘 #68: 見出しで分けず、既知/未知を問わず同じ
  // 見た目の1つのリストとして混ぜて表示する。
  const knownActorIds = new Set(items.map((i) => i.actor_id));
  const extraItems = remoteExtra.filter((r) => !r.actor_id || !knownActorIds.has(r.actor_id));

  const emptyMessage =
    kind === "following" ? t("profile:profilePage.followList.noFollowing") : t("profile:profilePage.followList.noFollowers");
  const remoteStillLoading = isRemoteFedi && remoteState === "loading";
  const isEmpty = items.length === 0 && extraItems.length === 0;

  if (isEmpty && !remoteStillLoading) {
    return <p className={panel.message}>{emptyMessage}</p>;
  }

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
      {extraItems.map((item) =>
        item.actor_id ? (
          <Link key={item.uri} to={profilePath(item.handle, item.domain)} className={styles.row}>
            <Avatar url={item.avatar_url} name={item.display_name || item.handle} size={40} />
            <div className={styles.names}>
              <span className={styles.displayName}>{item.display_name || item.handle}</span>
              <span className={styles.acct}>@{item.handle}@{item.domain}</span>
            </div>
          </Link>
        ) : (
          <Link key={item.uri} to={profilePath(item.handle, item.domain)} className={styles.row}>
            <Avatar url={undefined} name={item.handle} size={40} />
            <div className={styles.names}>
              <span className={styles.displayName}>@{item.handle}</span>
              <span className={styles.acct}>@{item.handle}@{item.domain}</span>
            </div>
          </Link>
        )
      )}
      {remoteStillLoading && <p className={panel.message}>{t("common:loading")}</p>}
      {isRemoteFedi && remoteState === "pending" && (
        <p className={panel.message}>{t("profile:profilePage.followList.remoteExtraPending")}</p>
      )}
    </div>
  );
}
