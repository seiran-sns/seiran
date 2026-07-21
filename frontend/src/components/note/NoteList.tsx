import { useTranslation } from "react-i18next";
import { Note } from "../../api/client";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import panel from "../common/Panel.module.css";
import styles from "./NoteList.module.css";
import NoteCard from "./NoteCard";

interface NoteListProps {
  notes: Note[];
  loading?: boolean;
  emptyMessage?: string;
  linkToDetail?: boolean;
  /** リアルタイム挿入されたばかりのノート ID。押し出しアニメーションを付与する（#37）。 */
  enteringIds?: Set<string>;
  /** 末尾の sentinel が画面内に入ったら過去分を追加取得する（無限スクロール）。未指定なら無効。 */
  onLoadMore?: () => void;
  hasMore?: boolean;
  loadingMore?: boolean;
}

export default function NoteList({
  notes,
  loading,
  emptyMessage,
  linkToDetail = true,
  enteringIds,
  onLoadMore,
  hasMore,
  loadingMore,
}: NoteListProps) {
  const { t } = useTranslation();
  const resolvedEmptyMessage = emptyMessage ?? t("home:noteList.emptyDefault");
  const sentinelRef = useInfiniteScrollSentinel<HTMLDivElement>(onLoadMore, hasMore);

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;
  if (notes.length === 0) return <p className={panel.message}>{resolvedEmptyMessage}</p>;
  return (
    <div>
      {notes.map((note) => (
        <div key={note.id} className={enteringIds?.has(note.id) ? styles.entering : undefined}>
          <NoteCard note={note} linkToDetail={linkToDetail} />
        </div>
      ))}
      {onLoadMore && hasMore && (
        <div ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </div>
      )}
    </div>
  );
}
