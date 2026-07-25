import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import NoteHoverPreview from "./NoteHoverPreview";
import styles from "./ReplyIndicator.module.css";

/**
 * リプライであることを示す ↩️ インジケータ（issue #20）。
 * マウスオーバーで返信先ポストをフェッチしてポップアップ表示する。
 * タイムライン・詳細画面の両方で使用する。
 */
export default function ReplyIndicator({ replyId }: { replyId: string }) {
  const { t } = useTranslation();

  return (
    <NoteHoverPreview noteId={replyId}>
      <Link
        to={`/notes/${replyId}`}
        className={styles.indicator}
        onClick={(e) => e.stopPropagation()}
        title={t("home:replyIndicator.goToOriginalTitle")}
      >
        <span aria-hidden>↩️</span> {t("home:replyIndicator.replyLabel")}
      </Link>
    </NoteHoverPreview>
  );
}
