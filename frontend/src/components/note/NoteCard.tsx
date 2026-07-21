import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, ApiError, getErrorMessage, Note, ReactionSummary } from "../../api/client";
import { acct, deliveryBadges, displayName, formatDate, profilePath, protocolBadge, visibilityBadge } from "../../lib/format";
import ReplyIndicator from "./ReplyIndicator";
import Avatar from "./Avatar";
import ReactionChips from "./ReactionChips";
import ReactionPicker from "./ReactionPicker";
import EmojiText from "./EmojiText";
import RichText from "./RichText";
import HlsVideo from "./HlsVideo";
import { useComposer } from "../../contexts/ComposerContext";
import { useAuth } from "../../contexts/AuthContext";
import { useToast } from "../../contexts/ToastContext";
import { ReactionUpdate, useStreamingContext } from "../../contexts/StreamingContext";
import styles from "./NoteCard.module.css";

/**
 * リアクションの楽観的更新を適用した新しい配列を返す。
 * 1投稿につき1ユーザー1リアクションまで（Misskey 準拠）なので、既に別の絵文字に
 * リアクション済みならまずそれを外してから、`reacting` なら新しい絵文字を付ける
 * （＝切り替え）。同じ絵文字を指定した場合は取消（トグルオフ）のみになる。
 */
export function optimisticSetReaction(
  reactions: ReactionSummary[],
  emoji: string,
  reacting: boolean
): ReactionSummary[] {
  const prevMine = reactions.find((r) => r.reactedByMe)?.emoji;
  let next = reactions;

  if (prevMine) {
    next = next
      .map((r) => (r.emoji === prevMine ? { ...r, count: r.count - 1, reactedByMe: false } : r))
      .filter((r) => r.count > 0);
  }

  if (reacting) {
    const existing = next.find((r) => r.emoji === emoji);
    next = existing
      ? next.map((r) => (r.emoji === emoji ? { ...r, count: r.count + 1, reactedByMe: true } : r))
      : [...next, { emoji, count: 1, reactedByMe: true }];
  }

  return next;
}

/**
 * WebSocket 経由で届いた `noteUpdated`（リアクション追加/切替/取消）を現在の表示に反映する。
 * サーバーから届く集計は閲覧者ごとの `reactedByMe` を含まないため、自分自身の操作
 * （`reactorActorId` が自分の actor_id と一致）ならその場で `reactedByMe` を再計算し、
 * 他人の操作ならローカルで既に把握している `reactedByMe` をそのまま引き継ぐ。
 */
export function applyReactionUpdate(
  reactions: ReactionSummary[],
  update: ReactionUpdate,
  myActorId: number | undefined
): ReactionSummary[] {
  const isMe = myActorId !== undefined && update.reactorActorId === myActorId;
  return update.reactions.map((r) => ({
    emoji: r.emoji,
    count: r.count,
    emojiUrl: r.emojiUrl,
    reactedByMe: isMe
      ? r.emoji === update.reactorEmoji
      : reactions.find((x) => x.emoji === r.emoji)?.reactedByMe ?? false,
  }));
}

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
  const { user } = useAuth();
  const { showError } = useToast();
  const { registerReaction } = useStreamingContext();
  const badge = protocolBadge(note.user.actorType);
  const delBadges = deliveryBadges(note);
  const visBadge = visibilityBadge(note);
  const [reposting, setReposting] = useState(false);
  const [unreposting, setUnreposting] = useState(false);
  const [reposted, setReposted] = useState(note.repostedByMe ?? false);
  const [reactions, setReactions] = useState<ReactionSummary[]>(note.reactions ?? []);
  const isSelf = note.user.actorType === "local" && !!user && user.username === note.user.username;
  const [pinned, setPinned] = useState(note.pinnedByMe ?? false);
  const [pinning, setPinning] = useState(false);
  // 1投稿につき1ユーザー1リアクションまでのため、切り替え中は他の絵文字操作も
  // まとめてロックする（個別絵文字ごとの pending 管理はしない）。
  const [reactionPending, setReactionPending] = useState(false);

  // 他ユーザー（または自分の別タブ/端末）によるリアクション追加/切替/取消をリアルタイム反映する。
  useEffect(() => {
    return registerReaction(note.id, (update) => {
      setReactions((prev) => applyReactionUpdate(prev, update, user?.actor_id));
    });
  }, [note.id, registerReaction, user?.actor_id]);

  const isPrivateRepostTarget = note.visibility === "followers_only" || note.visibility === "direct";

  async function handleRepost(e: React.MouseEvent) {
    e.stopPropagation();
    if (reposting || unreposting) return;

    if (reposted) {
      setUnreposting(true);
      try {
        await api.notes.deleteRepost(note.id);
        setReposted(false);
        onUnreposted?.();
      } catch (err) {
        showError(getErrorMessage(err));
      } finally {
        setUnreposting(false);
      }
      return;
    }

    if (isPrivateRepostTarget) return;

    setReposting(true);
    try {
      await api.notes.create("", true, true, [], undefined, note.id);
      setReposted(true);
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setReposted(true);
      } else if (err instanceof ApiError && err.status === 403) {
        showError(t("home:noteCard.privateRepostError"));
      } else {
        showError(getErrorMessage(err));
      }
    } finally {
      setReposting(false);
    }
  }

  async function toggleReaction(emoji: string) {
    if (reactionPending) return;
    const reacting = !(reactions.find((r) => r.emoji === emoji)?.reactedByMe ?? false);
    const prevReactions = reactions;

    setReactionPending(true);
    setReactions((prev) => optimisticSetReaction(prev, emoji, reacting));
    try {
      const res = reacting
        ? await api.notes.react(note.id, emoji)
        : await api.notes.unreact(note.id, emoji);
      setReactions(res.reactions);
    } catch (err) {
      setReactions(prevReactions);
      showError(getErrorMessage(err));
    } finally {
      setReactionPending(false);
    }
  }

  async function handleTogglePin(e: React.MouseEvent) {
    e.stopPropagation();
    if (pinning) return;
    setPinning(true);
    try {
      if (pinned) {
        await api.notes.unpin(note.id);
        setPinned(false);
      } else {
        await api.notes.pin(note.id);
        setPinned(true);
      }
    } catch (err) {
      showError(getErrorMessage(err));
    } finally {
      setPinning(false);
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

      {note.attachments && note.attachments.length > 0 && (
        <div className={styles.attachments}>
          {note.attachments.map((att, i) => {
            const isHls = att.mimeType === "application/vnd.apple.mpegurl" || att.mimeType === "application/x-mpegURL";
            if (att.mimeType.startsWith("video/") || isHls) {
              return (
                <HlsVideo
                  key={i}
                  src={att.url}
                  poster={att.thumbnailUrl}
                  isHls={isHls}
                  className={styles.attachImage}
                  onClick={(e) => e.stopPropagation()}
                />
              );
            }
            if (att.mimeType.startsWith("audio/")) {
              return (
                <audio
                  key={i}
                  src={att.url}
                  controls
                  className={styles.attachAudio}
                  onClick={(e) => e.stopPropagation()}
                />
              );
            }
            return (
              <a
                key={i}
                href={att.url}
                target="_blank"
                rel="noopener noreferrer"
                onClick={(e) => e.stopPropagation()}
              >
                <img src={att.url} alt="" className={styles.attachImage} loading="lazy" />
              </a>
            );
          })}
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

      <ReactionChips reactions={reactions} onToggle={toggleReaction} disabled={reactionPending} />

      <div className={styles.actions}>
        <button
          className={styles.actionBtn}
          onClick={(e) => {
            e.stopPropagation();
            openReply(note);
          }}
          title={t("home:noteCard.replyButton")}
        >
          💬 {t("home:noteCard.replyButton")}
        </button>
        <button
          className={`${styles.actionBtn} ${reposted ? styles.actionBtnActive : ""}`}
          onClick={handleRepost}
          disabled={reposting || unreposting || (isPrivateRepostTarget && !reposted)}
          title={
            isPrivateRepostTarget
              ? t("home:noteCard.repostDisabledTitle")
              : reposted
              ? t("home:noteCard.unrepostTitle")
              : t("home:noteCard.repostTitle")
          }
        >
          🔁{" "}
          {isPrivateRepostTarget
            ? t("home:noteCard.repostUnavailable")
            : reposted
            ? t("home:noteCard.reposted")
            : (reposting || unreposting)
            ? "..."
            : t("home:noteCard.repost")}
        </button>
        <ReactionPicker onPick={toggleReaction} disabled={reactionPending} />
        {isSelf && (
          <button
            className={`${styles.actionBtn} ${pinned ? styles.actionBtnActive : ""}`}
            onClick={handleTogglePin}
            disabled={pinning}
            title={pinned ? t("home:noteCard.unpinTitle") : t("home:noteCard.pinTitle")}
          >
            📌 {pinned ? t("home:noteCard.pinned") : pinning ? "..." : t("home:noteCard.pin")}
          </button>
        )}
      </div>
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
