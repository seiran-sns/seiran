import { useTranslation } from "react-i18next";
import { ReactionSummary } from "../../api/client";
import styles from "./ReactionChips.module.css";

interface ReactionChipsProps {
  reactions?: ReactionSummary[];
  /** チップクリック時に同じ絵文字でトグル（追加/取消・切替）する（未指定なら非インタラクティブ）。 */
  onToggle?: (emoji: string) => void;
  /** リアクション操作中は true。全チップのクリックを無効化する（1投稿1リアクションまでのため）。 */
  disabled?: boolean;
}

/** 届いたリアクションの集計チップ表示。クリックで同じ絵文字を自分も付ける/取り消す/切り替える。 */
export default function ReactionChips({ reactions, onToggle, disabled }: ReactionChipsProps) {
  const { t } = useTranslation();
  if (!reactions || reactions.length === 0) return null;
  return (
    <div className={styles.wrap}>
      {reactions.map((r) => {
        // カスタム絵文字（画像）はローカルからは送信できない（Unicode絵文字限定バリデーション）ため、
        // 自分がまだ付けていないカスタム絵文字チップはクリックしても必ず失敗する。無駄なリクエストを
        // 避けるため、その場合はクリックを無効化する（自分が既に付けている分の取消は許可する）。
        const addBlocked = !!r.emojiUrl && !r.reactedByMe;
        return (
          <button
            key={r.emoji}
            type="button"
            className={`${styles.chip} ${r.reactedByMe ? styles.chipActive : ""} ${addBlocked ? styles.chipReadonly : ""}`}
            title={
              addBlocked
                ? t("home:reactionChips.customEmojiBlocked")
                : r.reactedByMe
                  ? t("home:reactionChips.clickToRemove")
                  : t("home:reactionChips.clickToAdd")
            }
            disabled={!onToggle || disabled || addBlocked}
            onClick={(e) => {
              e.stopPropagation();
              onToggle?.(r.emoji);
            }}
          >
            {r.emojiUrl ? (
              <img className={styles.emojiImg} src={r.emojiUrl} alt={r.emoji} title={r.emoji} loading="lazy" />
            ) : (
              <span className={styles.emoji}>{r.emoji}</span>
            )}
            <span className={styles.count}>{r.count}</span>
          </button>
        );
      })}
    </div>
  );
}
