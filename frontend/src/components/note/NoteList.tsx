import { useCallback, useRef } from "react";
import { Note } from "../../api/client";
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
  emptyMessage = "投稿がありません。",
  linkToDetail = true,
  enteringIds,
  onLoadMore,
  hasMore,
  loadingMore,
}: NoteListProps) {
  const observerRef = useRef<IntersectionObserver | null>(null);

  // callback ref: sentinel の DOM 要素が生成・破棄されるたびに Reactが必ず呼ぶため、
  // useEffect + ref オブジェクトの組み合わせと違い、loading↔表示の切り替えで sentinel が
  // 新しい DOM ノードに置き換わった場合でも observer の再アタッチ漏れが起きない。
  // （tab切り替え直後に notes.length や hasMore がたまたま前後で同値になると、
  //   useEffect の依存配列だけでは新しい要素への observe し直しが発生しないバグがあった）
  const sentinelRef = useCallback(
    (el: HTMLDivElement | null) => {
      observerRef.current?.disconnect();
      observerRef.current = null;
      if (!el || !onLoadMore || !hasMore) return;
      const observer = new IntersectionObserver(
        (entries) => {
          if (entries[0]?.isIntersecting) onLoadMore();
        },
        { rootMargin: "200px" }
      );
      observer.observe(el);
      observerRef.current = observer;
    },
    [onLoadMore, hasMore]
  );

  if (loading) return <p className={panel.message}>読み込み中...</p>;
  if (notes.length === 0) return <p className={panel.message}>{emptyMessage}</p>;
  return (
    <div>
      {notes.map((note) => (
        <div key={note.id} className={enteringIds?.has(note.id) ? styles.entering : undefined}>
          <NoteCard note={note} linkToDetail={linkToDetail} />
        </div>
      ))}
      {onLoadMore && hasMore && (
        <div ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? "読み込み中…" : ""}
        </div>
      )}
    </div>
  );
}
