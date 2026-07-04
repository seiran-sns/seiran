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
}

export default function NoteList({
  notes,
  loading,
  emptyMessage = "投稿がありません。",
  linkToDetail = true,
  enteringIds,
}: NoteListProps) {
  if (loading) return <p className={panel.message}>読み込み中...</p>;
  if (notes.length === 0) return <p className={panel.message}>{emptyMessage}</p>;
  return (
    <div>
      {notes.map((note) => (
        <div key={note.id} className={enteringIds?.has(note.id) ? styles.entering : undefined}>
          <NoteCard note={note} linkToDetail={linkToDetail} />
        </div>
      ))}
    </div>
  );
}
