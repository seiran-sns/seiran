import { useTranslation } from "react-i18next";
import { ProfileFeedItem } from "../../api/client";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import panel from "../common/Panel.module.css";
import styles from "./NoteList.module.css";
import NoteCard from "./NoteCard";
import ReactionEventCard from "./ReactionEventCard";

interface ProfileFeedListProps {
  items: ProfileFeedItem[];
  loading?: boolean;
  emptyMessage?: string;
  linkToDetail?: boolean;
  onLoadMore?: () => void;
  hasMore?: boolean;
  loadingMore?: boolean;
}

/**
 * プロフィール「投稿」タブの投稿＋リアクションイベント混合リスト。`NoteList` と同じ骨格
 * （sentinel による無限スクロール）だが、`item.kind` に応じて `NoteCard`/`ReactionEventCard`
 * を出し分ける。
 */
export default function ProfileFeedList({
  items,
  loading,
  emptyMessage,
  linkToDetail = true,
  onLoadMore,
  hasMore,
  loadingMore,
}: ProfileFeedListProps) {
  const { t } = useTranslation();
  const resolvedEmptyMessage = emptyMessage ?? t("home:noteList.emptyDefault");
  const sentinelRef = useInfiniteScrollSentinel<HTMLDivElement>(onLoadMore, hasMore);

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;
  if (items.length === 0) return <p className={panel.message}>{resolvedEmptyMessage}</p>;
  return (
    <div>
      {items.map((item) =>
        item.kind === "note" ? (
          <div key={`note:${item.note.id}`}>
            <NoteCard note={item.note} linkToDetail={linkToDetail} />
          </div>
        ) : (
          <div key={`reaction:${item.event.id}`}>
            <ReactionEventCard event={item.event} />
          </div>
        ),
      )}
      {onLoadMore && hasMore && (
        <div ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </div>
      )}
    </div>
  );
}
