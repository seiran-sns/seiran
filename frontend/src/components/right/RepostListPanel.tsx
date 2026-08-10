import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { api, Note, RepostEntry, getErrorMessage } from "../../api/client";
import { formatDate } from "../../lib/format";
import Avatar from "../note/Avatar";
import panel from "../common/Panel.module.css";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./RepostListPanel.module.css";

interface RepostListPanelProps {
  /** リポスト元の実体。呼び出し側でリポスト元実体を渡すこと。 */
  note: Note;
}

/** ポスト詳細右ペインの「リポスト」タブ（#226）: 対象ポストをリポストしたユーザーと時刻の一覧。
 * fediで同一ユーザーから複数回リポストされていればその件数ぶんレコードが並ぶ。取り消し済みの
 * リポストも履歴として表示するが、詳細画面が存在しないためリンク化しない。 */
export default function RepostListPanel({ note }: RepostListPanelProps) {
  const { t } = useTranslation();
  const [reposts, setReposts] = useState<RepostEntry[] | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    setReposts(null);
    setError("");
    api.notes
      .reposts(note.id)
      .then((rows) => !cancelled && setReposts(rows))
      .catch((e) => !cancelled && setError(getErrorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [note.id]);

  if (error) return <p className={panel.message}>{error}</p>;
  if (reposts === null) return <p className={panel.message}>{t("common:loading")}</p>;

  if (reposts.length === 0) {
    return (
      <div className={panel.placeholder}>
        <TwemojiEmoji emoji="🔁" className={panel.placeholderIcon} />
        {t("home:noteDetailPage.noReposts")}
      </div>
    );
  }

  return (
    <ul className={styles.list}>
      {reposts.map((r) => {
        const row = (
          <>
            <Avatar url={r.user.avatarUrl} name={r.user.displayName || r.user.username} size={36} />
            <div className={styles.info}>
              <span className={styles.name}>{r.user.displayName || r.user.username}</span>
              <span className={styles.time}>
                {formatDate(r.createdAt)}
                {r.deleted && ` · ${t("home:noteDetailPage.repostUndone")}`}
              </span>
            </div>
          </>
        );
        return (
          <li key={r.id} className={styles.row}>
            {r.deleted ? (
              <span className={`${styles.link} ${styles.undone}`}>{row}</span>
            ) : (
              <Link to={`/notes/${r.id}`} className={styles.link}>
                {row}
              </Link>
            )}
          </li>
        );
      })}
    </ul>
  );
}
