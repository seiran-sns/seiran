import { ReactionSummary } from "../../api/client";
import { mediaUrl } from "../../utils/mediaProxy";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./MessageReactions.module.css";

/** メッセージ画面（DM）のリアクション表示（LINE風）。`ReactionChips`と異なり数字は
 * 表示せず、同じ絵文字が複数回付いた場合はその数だけ同じアイコンを並べる。 */
export default function MessageReactions({ reactions }: { reactions?: ReactionSummary[] }) {
  if (!reactions || reactions.length === 0) return null;
  return (
    <div className={styles.wrap}>
      {reactions.flatMap((r) =>
        Array.from({ length: r.count }, (_, i) => (
          <span className={styles.icon} key={`${r.emoji}-${i}`}>
            {r.emojiUrl ? (
              <img className={styles.emojiImg} src={mediaUrl(r.emojiUrl)} alt={r.emoji} loading="lazy" />
            ) : (
              <TwemojiEmoji emoji={r.emoji} className={styles.emojiImg} />
            )}
          </span>
        )),
      )}
    </div>
  );
}
