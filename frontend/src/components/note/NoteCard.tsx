import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Note } from "../../api/client";
import { acct, deliveryBadges, displayName, formatDate, profilePath, protocolBadge, visibilityBadge } from "../../lib/format";
import { useNoteCardActions } from "../../hooks/useNoteCardActions";
import ReplyIndicator from "./ReplyIndicator";
import Avatar from "./Avatar";
import EmojiText from "./EmojiText";
import RichText from "./RichText";
import NoteAttachments from "./NoteAttachments";
import NoteCardActions from "./NoteCardActions";
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
  const { t } = useTranslation();
  const { openReply } = useComposer();
  const badge = protocolBadge(note.user.actorType);
  const delBadges = deliveryBadges(note);
  const visBadge = visibilityBadge(note);

  const {
    isSelf,
    isPrivateRepostTarget,
    reactions,
    reactionPending,
    toggleReaction,
    reposted,
    reposting,
    unreposting,
    handleRepost,
    pinned,
    pinning,
    handleTogglePin,
  } = useNoteCardActions(note, onUnreposted);

  function goProfile(e: React.MouseEvent) {
    e.stopPropagation();
    navigate(profilePath(note.user.username, note.user.domain));
  }

  function handleReply(e: React.MouseEvent) {
    e.stopPropagation();
    openReply(note);
  }

  return (
    <>
      <div className={styles.header}>
        <button className={styles.userBtn} onClick={goProfile}>
          <Avatar url={note.user.avatarUrl} name={note.user.displayName || note.user.username} size={large ? 48 : 40} />
          <span className={styles.names}>
            <span className={styles.displayName}>
              <EmojiText text={displayName(note)} emojis={note.emojis} />
            </span>
            <span className={styles.acct}>
              {acct(note)}
              {badge && (
                <span className={styles.protoBadge} title={badge.label}>
                  {badge.icon}
                </span>
              )}
              {delBadges.map((b) => (
                <span key={b.icon} className={styles.protoBadge} title={b.label}>
                  {b.icon}
                </span>
              ))}
              {visBadge && (
                <span className={styles.protoBadge} title={visBadge.label}>
                  {visBadge.icon}
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
              {t("home:noteCard.quoteSourceLink")}
            </Link>
          )}
        </div>
      )}

      <p className={styles.body}>
        <RichText text={note.text} emojis={note.emojis} />
      </p>

      <NoteAttachments attachments={note.attachments} />

      {note.parentOriginalId && (
        <Link
          to={`/notes/${note.parentOriginalId}`}
          className={styles.originalLink}
          onClick={(e) => e.stopPropagation()}
          title={t("home:noteCard.originalLinkTitle")}
        >
          {t("home:noteCard.originalLinkText")}
        </Link>
      )}

      <NoteCardActions
        reactions={reactions}
        reactionPending={reactionPending}
        onToggleReaction={toggleReaction}
        onReply={handleReply}
        reposted={reposted}
        reposting={reposting}
        unreposting={unreposting}
        isPrivateRepostTarget={isPrivateRepostTarget}
        onRepost={handleRepost}
        isSelf={isSelf}
        pinned={pinned}
        pinning={pinning}
        onTogglePin={handleTogglePin}
      />
    </>
  );
}

export default function NoteCard({ note, linkToDetail = true, large = false }: NoteCardProps) {
  const { t } = useTranslation();
  const [hidden, setHidden] = useState(false);

  if (hidden) return null;

  if (note.renote) {
    const suffix = t("home:noteCard.repostedSuffix");
    return (
      <article className={`${styles.card} ${large ? styles.large : ""}`}>
        <div className={styles.rail}>
          🔁 <strong><EmojiText text={displayName(note)} emojis={note.emojis} /></strong>{" "}
          {t("home:noteCard.repostedConnector")}{" "}
          <Link to={`/notes/${note.id}`} className={styles.repostTime} onClick={(e) => e.stopPropagation()}>
            {formatDate(note.createdAt)}
          </Link>
          {suffix && <>{" "}{suffix}</>}
        </div>
        <PostContent note={note.renote} linkToDetail={linkToDetail} large={large} onUnreposted={() => setHidden(true)} />
      </article>
    );
  }

  // renoteId はあるが renote が欠落している場合、元ポストが非公開（プライベート/ひかえめ）で
  // 閲覧者から見えないケース（embed_renotes の可視性ガードによるもの）。
  if (note.renoteId) {
    return (
      <article className={`${styles.card} ${large ? styles.large : ""}`}>
        <div className={styles.rail}>
          🔁 <strong><EmojiText text={displayName(note)} emojis={note.emojis} /></strong>{" "}
          {t("home:noteCard.repostedNoLinkSuffix")}
        </div>
        <p className={styles.unavailableNote}>{t("home:noteCard.unavailableRepost")}</p>
      </article>
    );
  }

  return (
    <article className={`${styles.card} ${large ? styles.large : ""}`}>
      <PostContent note={note} linkToDetail={linkToDetail} large={large} />
    </article>
  );
}
