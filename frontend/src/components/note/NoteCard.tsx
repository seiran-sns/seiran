import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, Note } from "../../api/client";
import { acct, deliveryBadges, displayName, formatDate, profilePath, profileQuery, protocolBadge, visibilityBadge } from "../../lib/format";
import { useNoteCardActions } from "../../hooks/useNoteCardActions";
import { useAuth } from "../../contexts/AuthContext";
import { useToast } from "../../contexts/ToastContext";
import { setFollowStatus as setFollowStatusStore, useFollowStatus } from "../../stores/followStatusStore";
import { setPollState, usePollState } from "../../stores/pollVoteStore";
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

  const targetKey = profileQuery(note.user.username, note.user.domain);

  const isAuthorSelf = isSelf || (!!currentUser && currentUser.username === note.user.username && (!note.user.domain || note.user.domain === window.location.hostname));

  const [isHovered, setIsHovered] = useState(false);
  const [showContent, setShowContent] = useState(!note.contentWarning);
  const [loadingStatus, setLoadingStatus] = useState(false);
  const [followActionPending, setFollowActionPending] = useState(false);
  const sharedPollState = usePollState(note.id, note.poll);
  const poll = sharedPollState?.poll;
  const pollVoted = (sharedPollState?.votedByMe.length ?? 0) > 0;
  const [pollResults, setPollResults] = useState(false);
  const [pollSelection, setPollSelection] = useState<number[]>([]);
  const [pollPending, setPollPending] = useState(false);
  const [pollRenderedAt] = useState(() => Date.now());
  // フォロー状態は共有ストア（stores/followStatusStore）を参照する。プロフィール画面や
  // 同一ユーザーの他ポストのフォロースイッチと状態が一本化されるため、一方で操作するか
  // WebSocket の `followAccepted`（StreamingContext）を受けるだけで全ての表示に伝播する。
  // ストアに未登録（undefined）なら「まだ取得していない」ことを意味する。
  const followStatus = useFollowStatus(targetKey) ?? null;

  const pollClosed = !!poll && [poll.closed, poll.endTime]
    .filter(Boolean)
    .some((value) => new Date(value!).getTime() <= pollRenderedAt);

  async function submitPollVote(indexes: number[]) {
    if (!currentUser) {
      showError("投票するにはログインが必要です");
      return;
    }
    setPollPending(true);
    try {
      const result = await api.notes.votePoll(note.id, indexes);
      setPollState(note.id, { poll: result.poll, votedByMe: indexes });
      setPollResults(true);
    } catch (error) {
      showError(getErrorMessage(error));
    } finally {
      setPollPending(false);
    }
  }

  function handleMouseEnter() {
    setIsHovered(true);
    if (!isAuthorSelf && followStatus === null && !loadingStatus) {
      setLoadingStatus(true);
      api.users.profile(targetKey)
        .then((p) => setFollowStatusStore(targetKey, p.follow_status))
        .catch(() => setFollowStatusStore(targetKey, "not_following"))
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
        setFollowStatusStore(targetKey, res.status === "accepted" ? "accepted" : "pending");
      } else {
        await api.follows.delete(targetKey);
        setFollowStatusStore(targetKey, "not_following");
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

      {note.contentWarning && (
        <div className={styles.contentWarningWrap}>
          <p className={styles.contentWarningText}>⚠️ {note.contentWarning}</p>
          <button className={styles.contentWarningToggle} onClick={(e) => {
            e.stopPropagation();
            setShowContent((shown) => !shown);
          }}>
            {showContent ? "隠す" : "表示"}
          </button>
        </div>
      )}
      {showContent && (
        <p className={styles.body}>
          <RichText text={note.text} emojis={note.emojis} />
        </p>
      )}

      <NoteAttachments attachments={note.attachments} />

      {poll && (
        <div className={styles.poll}>
          {(pollResults || pollVoted || pollClosed) ? poll.options.map((option, index) => (
            <div className={`${styles.pollOption} ${sharedPollState?.votedByMe.includes(index) ? styles.pollOptionVoted : ""}`} key={option.name}>
              <span>{sharedPollState?.votedByMe.includes(index) && "✓ "}{option.name}</span>
              <span>{option.votes}票</span>
            </div>
          )) : poll.options.map((option, index) => poll.multiple ? (
            <label className={styles.pollChoice} key={option.name}>
              <input type="checkbox" checked={pollSelection.includes(index)} disabled={pollPending}
                onChange={(e) => setPollSelection((selected) => e.target.checked
                  ? [...selected, index]
                  : selected.filter((i) => i !== index))} />
              <span>{option.name}</span>
            </label>
          ) : (
            <button className={styles.pollChoice} key={option.name} disabled={pollPending}
              onClick={(e) => { e.stopPropagation(); void submitPollVote([index]); }}>
              {option.name}
            </button>
          ))}
          {!pollVoted && !pollClosed && (
            <div className={styles.pollControls}>
              {poll.multiple && !pollResults && (
                <button disabled={pollPending || pollSelection.length === 0}
                  onClick={(e) => { e.stopPropagation(); void submitPollVote(pollSelection); }}>
                  回答
                </button>
              )}
              <button onClick={(e) => { e.stopPropagation(); setPollResults((shown) => !shown); }}>
                {pollResults ? "回答に戻る" : "結果を見る"}
              </button>
            </div>
          )}
        </div>
      )}

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
        noteId={note.id}
        subjectActorId={String(note.user.id)}
        subjectLabel={`@${note.user.username} の投稿`}
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
