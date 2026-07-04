import { ReactionSummary } from "../../api/client";
import styles from "./ReactionChips.module.css";

/** 外部から届いたリアクション/いいねの集計チップ表示（issue #22・現状は表示のみ）。 */
export default function ReactionChips({ reactions }: { reactions?: ReactionSummary[] }) {
  if (!reactions || reactions.length === 0) return null;
  return (
    <div className={styles.wrap}>
      {reactions.map((r, i) => (
        <span key={i} className={styles.chip} title={r.emoji}>
          <span className={styles.emoji}>{r.emoji}</span>
          <span className={styles.count}>{r.count}</span>
        </span>
      ))}
    </div>
  );
}
