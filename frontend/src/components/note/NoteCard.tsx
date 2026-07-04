import { Link, useNavigate } from "react-router-dom";
import { Note } from "../../api/client";
import { acct, displayName, formatDate, profileQuery, protocolBadge } from "../../lib/format";
import ReplyIndicator from "./ReplyIndicator";
import Avatar from "./Avatar";
import styles from "./NoteCard.module.css";

interface NoteCardProps {
  note: Note;
  /** クリックでポスト詳細へ遷移させるか（デフォルト true）。 */
  linkToDetail?: boolean;
}

export default function NoteCard({ note, linkToDetail = true }: NoteCardProps) {
  const navigate = useNavigate();
  const badge = protocolBadge(note.user.actorType);

  function goProfile(e: React.MouseEvent) {
    e.stopPropagation();
    navigate(`/profile?q=${encodeURIComponent(profileQuery(note.user.username, note.user.domain))}`);
  }

  return (
    <article className={styles.card}>
      {note.renoteId && (
        <div className={styles.rail}>
          🔁 リノート
        </div>
      )}

      <div className={styles.header}>
        <button className={styles.userBtn} onClick={goProfile}>
          <Avatar url={note.user.avatarUrl} name={note.user.displayName || note.user.username} size={40} />
          <span className={styles.names}>
            <span className={styles.displayName}>{displayName(note)}</span>
            <span className={styles.acct}>
              {acct(note)}
              {badge && (
                <span className={styles.protoBadge} title={badge.label}>
                  {badge.icon}
                </span>
              )}
            </span>
          </span>
        </button>
        {linkToDetail ? (
          <Link to={`/notes/${note.id}`} className={styles.time} onClick={(e) => e.stopPropagation()}>
            <time>{formatDate(note.createdAt)}</time>
          </Link>
        ) : (
          <time className={styles.time}>{formatDate(note.createdAt)}</time>
        )}
      </div>

      {(note.replyId || note.quoteId) && (
        <div className={styles.relations}>
          {note.replyId && <ReplyIndicator replyId={note.replyId} />}
          {note.quoteId && (
            <Link to={`/notes/${note.quoteId}`} className={styles.relLink} onClick={(e) => e.stopPropagation()}>
              ❝ 引用元
            </Link>
          )}
        </div>
      )}

      <p className={styles.body}>{note.text}</p>

      {note.attachments && note.attachments.length > 0 && (
        <div className={styles.attachments}>
          {note.attachments.map((att, i) => (
            <a
              key={i}
              href={att.url}
              target="_blank"
              rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
            >
              <img src={att.url} alt="" className={styles.attachImage} loading="lazy" />
            </a>
          ))}
        </div>
      )}

      {note.parentOriginalId && (
        <Link
          to={`/notes/${note.parentOriginalId}`}
          className={styles.originalLink}
          onClick={(e) => e.stopPropagation()}
          title="この投稿はブリッジ/連合を経由した重複です。本尊のオリジナル投稿へ移動します。"
        >
          🀄 本尊のオリジナル投稿を見る
        </Link>
      )}
    </article>
  );
}
