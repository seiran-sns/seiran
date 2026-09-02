import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import NoteHoverPreview from "./NoteHoverPreview";
import styles from "./ReplyIndicator.module.css";

interface ReplyIndicatorProps {
  replyId: string;
  /** 指定時、通常の詳細ページ遷移の代わりにこのハンドラを呼ぶ（ポスト詳細画面の
   * スレッド遡り表示で、遷移せずその場に返信先ポストを積み上げるために使う）。 */
  onClimb?: (replyId: string) => void;
}

/**
 * リプライであることを示す ↩️ インジケータ（issue #20）。
 * マウスオーバーで返信先ポストをフェッチしてポップアップ表示する。
 * タイムライン・詳細画面の両方で使用する。
 */
export default function ReplyIndicator({ replyId, onClimb }: ReplyIndicatorProps) {
  const { t } = useTranslation();

  return (
    <NoteHoverPreview noteId={replyId}>
      <Link
        to={`/notes/${replyId}`}
        className={styles.indicator}
        onClick={(e) => {
          e.stopPropagation();
          if (onClimb) {
            e.preventDefault();
            onClimb(replyId);
          }
        }}
        title={t("home:replyIndicator.goToOriginalTitle")}
      >
        <span aria-hidden>↩️</span> {t("home:replyIndicator.replyLabel")}
      </Link>
    </NoteHoverPreview>
  );
}
