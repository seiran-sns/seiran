import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, Note } from "../../api/client";
import { acct, deliveryBadges, displayName, formatDate, profilePath, protocolBadge, visibilityBadge } from "../../lib/format";
import { useNoteCardActions } from "../../hooks/useNoteCardActions";
import { useAuth } from "../../contexts/AuthContext";
import { useToast } from "../../contexts/ToastContext";
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

function PostContent({ note, linkToDetail, large = false, onUnreposted, onDeleted }: {
  note: Note;
  linkToDetail: boolean;
  large?: boolean;
  onUnreposted?: () => void;
  onDeleted?: () => void;
}) {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const { showError } = useToast();
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
    deleting,
    handleDelete,
  } = useNoteCardActions(note, onUnreposted, onDeleted);

  const targetKey = note.user.domain && note.user.domain !== window.location.hostname
    ? `${note.user.username}@${note.user.domain}`
    : note.user.username;

  const isAuthorSelf = isSelf || (!!currentUser && currentUser.username === note.user.username && (!note.user.domain || note.user.domain === window.location.hostname));

  const [isHovered, setIsHovered] = useState(false);
  const [followStatus, setFollowStatus] = useState<"not_following" | "pending" | "accepted" | null>(null);
  const [loadingStatus, setLoadingStatus] = useState(false);
  const [followActionPending, setFollowActionPending] = useState(false);

  function handleMouseEnter() {
    setIsHovered(true);
    if (!isAuthorSelf && followStatus === null && !loadingStatus) {
      setLoadingStatus(true);
      api.users.profile(targetKey)
        .then((p) => setFollowStatus(p.follow_status))
        .catch(() => setFollowStatus("not_following"))
        .finally(() => setLoadingStatus(false));
    }
  }

  function handleMouseLeave() {
    setIsHovered(false);
  }

  async function handleToggleFollow(e: React.MouseEvent) {
    e.stopPropagation();
    if (followActionPending || isAuthorSelf) return;

    setFollowActionPending(true);
    const current = followStatus ?? "not_following";

    try {
      if (current === "not_following") {
        const res = await api.follows.create(targetKey);
        setFollowStatus(res.status === "accepted" ? "accepted" : "pending");
      } else {
        await api.follows.delete(targetKey);
        setFollowStatus("not_following");
      }
    } catch (err) {
      showError(getErrorMessage(err));
    } finally {
      setFollowActionPending(false);
    }
  }

  function getFollowLabel(status: "not_following" | "pending" | "accepted" | null): string {
    if (status === "accepted") return t("home:noteCard.following");
    if (status === "pending") return t("home:noteCard.followPending");
    return t("home:noteCard.notFollowing");
  }

  function goProfile(e: React.MouseEvent) {
    e.stopPropagation();
    navigate(profilePath(note.user.username, note.user.domain));
  }

  function handleReply(e?: React.MouseEvent) {
    e?.stopPropagation();
    openReply(note);
  }

  return (
    <>
      <div className={styles.header}>
        <div
          className={styles.userContainer}
          onMouseEnter={handleMouseEnter}
          onMouseLeave={handleMouseLeave}
        >
          {isHovered && !isAuthorSelf && (
            <div className={styles.followWidgetPopover} onClick={(e) => e.stopPropagation()}>
              <span className={`${styles.followWidgetLabel} ${styles[`status_${followStatus ?? "not_following"}`]}`}>
                {loadingStatus ? t("common:loading") : getFollowLabel(followStatus)}
              </span>
              <button
                type="button"
                className={`${styles.followSwitch} ${styles[`switch_${followStatus ?? "not_following"}`]}`}
                onClick={handleToggleFollow}
                disabled={followActionPending || loadingStatus}
                title={getFollowLabel(followStatus)}
                aria-label={getFollowLabel(followStatus)}
              >
                <span className={styles.followSwitchKnob} />
              </button>
            </div>
          )}

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
        </div>
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
        deleting={deleting}
        onDelete={handleDelete}
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
        <PostContent
          note={note.renote}
          linkToDetail={linkToDetail}
          large={large}
          onUnreposted={() => setHidden(true)}
          onDeleted={() => setHidden(true)}
        />
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
      <PostContent note={note} linkToDetail={linkToDetail} large={large} onDeleted={() => setHidden(true)} />
    </article>
  );
}
