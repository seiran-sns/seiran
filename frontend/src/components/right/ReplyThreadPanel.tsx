import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, Note, getErrorMessage } from "../../api/client";
import NoteCard from "../note/NoteCard";
import panel from "../common/Panel.module.css";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./ReplyThreadPanel.module.css";

interface ReplyThreadPanelProps {
  /** ツリーの根となるポスト。リポストの場合は呼び出し側でリポスト元実体を渡すこと。 */
  note: Note;
}

const MAX_INDENT_DEPTH = 6;

/** `notes`（対象ポストへの直系リプライ・引用の再帰取得結果）から、id → 直接の子ノート配列のマップを組み立てる。
 * 各ノートの親は `replyId` を優先し、無ければ `quoteId` を使う（返信兼引用の場合は返信関係をツリーの軸にする）。
 * サーバーは `ORDER BY depth, id` で返すため、子配列は既に古い順で並んでいる。 */
function buildChildrenMap(notes: Note[]): Map<string, Note[]> {
  const known = new Set(notes.map((n) => n.id));
  const children = new Map<string, Note[]>();
  for (const n of notes) {
    const parentId =
      n.replyId && known.has(n.replyId)
        ? n.replyId
        : n.quoteId && known.has(n.quoteId)
          ? n.quoteId
          : (n.replyId ?? n.quoteId);
    if (!parentId) continue;
    const list = children.get(parentId);
    if (list) list.push(n);
    else children.set(parentId, [n]);
  }
  return children;
}

function ReplyNode({
  note,
  children,
  depth,
}: {
  note: Note;
  children: Map<string, Note[]>;
  depth: number;
}) {
  const kids = children.get(note.id) ?? [];
  return (
    <div
      className={styles.node}
      style={{ marginLeft: Math.min(depth, MAX_INDENT_DEPTH) * 14 }}
    >
      <NoteCard note={note} />
      {kids.map((k) => (
        <ReplyNode key={k.id} note={k} children={children} depth={depth + 1} />
      ))}
    </div>
  );
}

/** ポスト詳細右ペインの「返信」タブ（#226）: 対象ポストへの直系リプライ・引用を再帰的にツリー表示する。 */
export default function ReplyThreadPanel({ note }: ReplyThreadPanelProps) {
  const { t } = useTranslation();
  const [notes, setNotes] = useState<Note[] | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    setNotes(null);
    setError("");
    api.notes
      .replies(note.id)
      .then((rows) => !cancelled && setNotes(rows))
      .catch((e) => !cancelled && setError(getErrorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [note.id]);

  if (error) return <p className={panel.message}>{error}</p>;
  if (notes === null) return <p className={panel.message}>{t("common:loading")}</p>;

  const children = buildChildrenMap(notes);
  const topLevel = children.get(note.id) ?? [];

  if (topLevel.length === 0) {
    return (
      <div className={panel.placeholder}>
        <TwemojiEmoji emoji="💬" className={panel.placeholderIcon} />
        {t("home:noteDetailPage.noReplies")}
      </div>
    );
  }

  return (
    <div>
      {topLevel.map((n) => (
        <ReplyNode key={n.id} note={n} children={children} depth={0} />
      ))}
    </div>
  );
}
