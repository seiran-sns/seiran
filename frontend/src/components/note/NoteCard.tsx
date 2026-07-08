import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, ApiError, Note } from "../../api/client";
import { acct, displayName, formatDate, profilePath, protocolBadge } from "../../lib/format";
import ReplyIndicator from "./ReplyIndicator";
import Avatar from "./Avatar";
import ReactionChips from "./ReactionChips";
import { useComposer } from "../../contexts/ComposerContext";
import styles from "./NoteCard.module.css";

interface NoteCardProps {
  note: Note;
  /** クリックでポスト詳細へ遷移させるか（デフォルト true）。 */
  linkToDetail?: boolean;
  /** 主役ポスト（ポスト詳細画面）用の大型表示（#43）。文字・アバターを拡大する。 */
  large?: boolean;
}

function PostContent({ note, linkToDetail, large = false, onUnreposted }: {
  note: Note;
  linkToDetail: boolean;
  large?: boolean;
  onUnreposted?: () => void;
}) {
  const navigate = useNavigate();
  const { openReply } = useComposer();
  const badge = protocolBadge(note.user.actorType);
  const [reposting, setReposting] = useState(false);
  const [unreposting, setUnreposting] = useState(false);
  const [reposted, setReposted] = useState(note.repostedByMe ?? false);

  async function handleRepost(e: React.MouseEvent) {
    e.stopPropagation();
    if (reposting || unreposting) return;

    if (reposted) {
      setUnreposting(true);
      try {
        await api.notes.deleteRepost(note.id);
        setReposted(false);
        onUnreposted?.();
      } catch {
        // エラー時は何もしない
      } finally {
        setUnreposting(false);
      }
      return;
    }

    setReposting(true);
    try {
      await api.notes.create("", true, true, [], undefined, note.id);
      setReposted(true);
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setReposted(true);
      }
    } finally {
      setReposting(false);
    }
  }

  function goProfile(e: React.MouseEvent) {
    e.stopPropagation();
    navigate(profilePath(note.user.username, note.user.domain));
  }

  return (
    <>
      <div className={styles.header}>
        <button className={styles.userBtn} onClick={goProfile}>
          <Avatar url={note.user.avatarUrl} name={note.user.displayName || note.user.username} size={large ? 48 : 40} />
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

      <ReactionChips reactions={note.reactions} />

      <div className={styles.actions}>
        <button
          className={styles.actionBtn}
          onClick={(e) => {
            e.stopPropagation();
            openReply(note);
          }}
          title="返信"
        >
          💬 返信
        </button>
        <button
          className={`${styles.actionBtn} ${reposted ? styles.actionBtnActive : ""}`}
          onClick={handleRepost}
          disabled={reposting || unreposting}
          title={reposted ? "リポスト解除" : "リポスト"}
        >
          🔁 {reposted ? "リポスト済み" : (reposting || unreposting) ? "..." : "リポスト"}
        </button>
      </div>
    </>
  );
}

export default function NoteCard({ note, linkToDetail = true, large = false }: NoteCardProps) {
  const [hidden, setHidden] = useState(false);

  if (hidden) return null;

  if (note.renote) {
    return (
      <article className={`${styles.card} ${large ? styles.large : ""}`}>
        <div className={styles.rail}>
          🔁 <strong>{displayName(note)}</strong> が{" "}
          <Link to={`/notes/${note.id}`} className={styles.repostTime} onClick={(e) => e.stopPropagation()}>
            {formatDate(note.createdAt)}
          </Link>{" "}
          にリポスト
        </div>
        <PostContent note={note.renote} linkToDetail={linkToDetail} large={large} onUnreposted={() => setHidden(true)} />
      </article>
    );
  }

  return (
    <article className={`${styles.card} ${large ? styles.large : ""}`}>
      <PostContent note={note} linkToDetail={linkToDetail} large={large} />
    </article>
  );
}
