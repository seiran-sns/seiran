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
  /** このノートIDの直前に「取りこぼし区間」の区切り（二重波線）を表示する。 */
  gapBeforeIds?: Set<string>;
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
  gapBeforeIds,
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
        <div key={note.id}>
          {gapBeforeIds?.has(note.id) && (
            <div className={styles.gap} role="separator" aria-label={t("home:noteList.gapLabel")} />
          )}
          {/* data-timeline-id はスクロール位置をポスト基準で覚えるため（lib/timelineScroll.ts）。
              区切りの下のカード自体に付け、区切りが後から挟まってもカードの位置で合わせる。 */}
          <div className={enteringIds?.has(note.id) ? styles.entering : undefined} data-timeline-id={note.id}>
            <NoteCard note={note} linkToDetail={linkToDetail} />
          </div>
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
