import { Note } from "../../api/types";
import EmojiText from "./EmojiText";
import RichHtml from "./RichHtml";

/** CW（閲覧注意）の警告文。バックエンドが変換したサニタイズ済みHTML（リモートFedi投稿の
 * 警告文に含まれるMFMのタグ・`$[ruby]`等）があればそれを描画し、無ければプレーンテキストとして
 * 描画する。ラップ要素を持たない（呼び出し側の要素のスタイルをそのまま活かすため）。 */
export default function ContentWarningText({
  note,
}: {
  note: Pick<Note, "contentWarning" | "contentWarningHtml" | "emojis">;
}) {
  if (note.contentWarningHtml) {
    return <RichHtml html={note.contentWarningHtml} emojis={note.emojis} />;
  }
  return <EmojiText text={note.contentWarning ?? ""} emojis={note.emojis} />;
}
