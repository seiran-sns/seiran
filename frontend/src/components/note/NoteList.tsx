import { Note } from "../../api/client";
import panel from "../common/Panel.module.css";
import NoteCard from "./NoteCard";

interface NoteListProps {
  notes: Note[];
  loading?: boolean;
  emptyMessage?: string;
  linkToDetail?: boolean;
}

export default function NoteList({
  notes,
  loading,
  emptyMessage = "投稿がありません。",
  linkToDetail = true,
}: NoteListProps) {
  if (loading) return <p className={panel.message}>読み込み中...</p>;
  if (notes.length === 0) return <p className={panel.message}>{emptyMessage}</p>;
  return (
    <div>
      {notes.map((note) => (
        <NoteCard key={note.id} note={note} linkToDetail={linkToDetail} />
      ))}
    </div>
  );
}
