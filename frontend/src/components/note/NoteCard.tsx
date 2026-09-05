import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, Note } from "../../api/client";
import {
  acct,
  deliveryBadges,
  displayName,
  formatDate,
  profilePath,
  profileQuery,
  protocolBadge,
  remoteServerBadgeInfo,
  visibilityBadge,
} from "../../lib/format";
import { useNoteCardActions } from "../../hooks/useNoteCardActions";
import { useFollowHoverSwitch } from "../../hooks/useFollowHoverSwitch";
import { useAuth } from "../../contexts/AuthContext";
import { useToast } from "../../contexts/ToastContext";
import { seedRelationshipIfAbsent } from "../../stores/userRelationshipStore";
import { setPollState, usePollState } from "../../stores/pollVoteStore";
import ReplyIndicator from "./ReplyIndicator";
import PendingReferenceIndicator from "./PendingReferenceIndicator";
import Avatar from "./Avatar";
import EmojiText from "./EmojiText";
import TwemojiEmoji from "../common/TwemojiEmoji";
import RichHtml from "./RichHtml";
import RichText from "./RichText";
import NoteAttachments from "./NoteAttachments";
import LinkCard from "./LinkCard";
import PollCountdown from "./PollCountdown";
import NoteCardActions from "./NoteCardActions";
import ReactionChips from "./ReactionChips";
import UserContextMenu from "./UserContextMenu";
import UserHoverPopover from "./UserHoverPopover";
import { UserRelationshipTarget } from "../../hooks/useUserRelationshipMenu";
import { useComposer } from "../../contexts/ComposerContext";
import blueskyLogo from "../../assets/bluesky-logo.svg";
import fediverseLogo from "../../assets/fediverse-logo.svg";
import { mediaUrl } from "../../utils/mediaProxy";
import styles from "./NoteCard.module.css";

interface NoteCardProps {
  note: Note;
  /** クリックでポスト詳細へ遷移させるか（デフォルト true）。 */
  linkToDetail?: boolean;
  /** 主役ポスト（ポスト詳細画面）用の大型表示（#43）。文字・アバターを拡大する。 */
  large?: boolean;
  /** 返信先ポスト（ポスト詳細画面のスレッド遡り表示）用の小型表示。文字・アバターを縮小する。
   * largeと同時指定はしない想定。 */
  small?: boolean;
  /** `#open_cw` 付きURLでの遷移時、CWを開いた状態で初期表示する（#229）。 */
  forceOpenCw?: boolean;
  /** 指定時、返信インジケータ（↩️ 返信）クリック時に詳細ページへ遷移する代わりにこれを呼ぶ
   * （スレッド遡り表示で、その場に返信先ポストをさらに積み上げるために使う）。 */
  onReplyIndicatorClick?: (replyId: string) => void;
}

/** 引用元を1段だけ表示する共通カード。引用元の `quoteId` はバッジだけ表示し、
 * `quote.quote` を描画しないことで引用の引用を再帰させない。 */
function QuoteCard({ note }: { note: Note }) {
  const { t } = useTranslation();
  const [showContent, setShowContent] = useState(!note.contentWarning);

  return (
    <section className={styles.quoteCard} onClick={(e) => e.stopPropagation()}>
      <div className={styles.quoteHeader}>
        <Link
          to={profilePath(note.user.username, note.user.domain)}
          className={styles.quoteUser}
        >
          <Avatar
            url={note.user.avatarUrl}
            name={note.user.displayName || note.user.username}
            size={30}
          />
          <span className={styles.quoteNames}>
            <strong>
              <EmojiText text={displayName(note)} emojis={note.emojis} />
            </strong>
            <span>{acct(note)}</span>
          </span>
        </Link>
        <Link to={`/notes/${note.id}`} className={styles.time}>
          <time>{formatDate(note.createdAt)}</time>
        </Link>
      </div>

      <div className={styles.quoteRelations}>
        {note.replyId && <ReplyIndicator replyId={note.replyId} />}
        {note.quoteId && (
          <span className={styles.quoteBadge}>
            {t("home:noteCard.hasQuote")}
          </span>
        )}
      </div>

      {note.contentWarning && (
        <div className={styles.quoteContentWarning}>
          <span>
            <TwemojiEmoji emoji="⚠️" /> <EmojiText text={note.contentWarning} emojis={note.emojis} />
          </span>
          <button
            type="button"
            onClick={() => setShowContent((shown) => !shown)}
          >
            {showContent
              ? t("home:noteCard.hideContent")
              : t("home:noteCard.showContent")}
          </button>
        </div>
      )}
      {showContent && (
        <>
          <p className={styles.quoteBody}>
            {note.contentHtml ? (
              <RichHtml html={note.contentHtml} emojis={note.emojis} />
            ) : (
              <RichText text={note.text} emojis={note.emojis} />
            )}
          </p>
          <NoteAttachments attachments={note.attachments} />
          {note.linkCards.map((card) => (
            <LinkCard key={card.url} card={card} indent={false} />
          ))}
          {note.poll && (
            <div className={styles.quotePoll}>
              {note.poll.options.map((option) => (
                <div className={styles.pollOption} key={option.name}>
                  <span>{option.name}</span>
                  <span>{t("home:noteCard.votes", { count: option.votes })}</span>
                </div>
              ))}
            </div>
          )}
        </>
      )}
      <div className={styles.quoteReactions}>
        <ReactionChips noteId={note.id} reactions={note.reactions} indent={false} />
      </div>
    </section>
  );
}

function PostContent({
  note,
  linkToDetail,
  large = false,
  small = false,
  onUnreposted,
  onDeleted,
  forceOpenCw = false,
  onReplyIndicatorClick,
}: {
  note: Note;
  linkToDetail: boolean;
  large?: boolean;
  small?: boolean;
  onUnreposted?: () => void;
  onDeleted?: () => void;
  forceOpenCw?: boolean;
  onReplyIndicatorClick?: (replyId: string) => void;
}) {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const { showError } = useToast();
  const { openReply, openQuote } = useComposer();
  const badge = protocolBadge(note.user.actorType);
  const delBadges = deliveryBadges(note);
  const visBadge = visibilityBadge(note);

  const {
    isSelf,
    isPrivateRepostTarget,
    isPrivateQuoteTarget,
    isGateReplyBlocked,
    isGateQuoteBlocked,
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

  const isAuthorSelf =
    isSelf ||
    (!!currentUser &&
      currentUser.username === note.user.username &&
      (!note.user.domain || note.user.domain === window.location.hostname));

  const authorTarget: UserRelationshipTarget = {
    username: note.user.username,
    domain: note.user.domain,
    actorId: String(note.user.id),
    reportLabel: `@${note.user.username}${note.user.domain ? `@${note.user.domain}` : ""}`,
  };

  const [showContent, setShowContent] = useState(!note.contentWarning || forceOpenCw);
  // pending参照が「取り込む」で解決された場合のローカル反映（#234）。
  const [resolvedReplyId, setResolvedReplyId] = useState<string | null>(null);
  const [resolvedQuote, setResolvedQuote] = useState<Note | null>(null);
  const effectiveReplyId = resolvedReplyId ?? note.replyId;

  async function handleQuoteResolved(resolvedId: string) {
    try {
      setResolvedQuote(await api.notes.get(resolvedId));
    } catch {
      // 取得に失敗しても致命的ではない（引用元IDは解決済みのため次回リロードでは表示される）
    }
  }
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
  const {
    followStatus,
    isHovered,
    loadingStatus,
    followActionPending,
    handleMouseEnter,
    handleMouseLeave,
    handleToggleFollow,
  } = useFollowHoverSwitch(authorTarget, isAuthorSelf);

  // タイムラインAPIレスポンス（home/local/social/global）は note.user に閲覧者との
  // 関係（フォロー状態・ミュート・ブロック・リポストミュート）を事前付与済みのため、
  // ストア未登録ならそれを初期値としてシードする（既存の値があれば上書きしない、
  // stores/userRelationshipStore.ts の seedRelationshipIfAbsent 参照）。
  useEffect(() => {
    if (isAuthorSelf || note.user.followStatus === undefined) return;
    seedRelationshipIfAbsent(targetKey, {
      followStatus: note.user.followStatus,
      isMuted: note.user.isMuted ?? false,
      isBlocking: note.user.isBlocking ?? false,
      isBlockedBy: note.user.isBlockedBy ?? false,
      isRepostMuted: note.user.isRepostMuted ?? false,
    });
  }, [
    targetKey,
    isAuthorSelf,
    note.user.followStatus,
    note.user.isMuted,
    note.user.isBlocking,
    note.user.isBlockedBy,
    note.user.isRepostMuted,
  ]);

  const pollClosed =
    !!poll &&
    [poll.closed, poll.endTime]
      .filter(Boolean)
      .some((value) => new Date(value!).getTime() <= pollRenderedAt);

  async function submitPollVote(indexes: number[]) {
    if (!currentUser) {
      showError(t("home:noteCard.pollLoginRequired"));
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

  function handleReply(e?: React.MouseEvent) {
    e?.stopPropagation();
    if (isGateReplyBlocked) {
      showError(t("home:noteCard.replyGateError"));
      return;
    }
    openReply(note);
  }

  function handleQuote(e?: React.MouseEvent) {
    e?.stopPropagation();
    if (isPrivateQuoteTarget) {
      showError(t("home:noteCard.privateQuoteError"));
      return;
    }
    if (isGateQuoteBlocked) {
      showError(t("home:noteCard.quoteGateError"));
      return;
    }
    openQuote(note);
  }

  const remoteInfo = remoteServerBadgeInfo({
    actorType: note.user.actorType,
    domain: note.user.domain,
    instance: note.user.instance,
  });

  return (
    <>
      <div className={styles.header}>
        <div
          className={styles.userContainer}
          onMouseEnter={handleMouseEnter}
          onMouseLeave={handleMouseLeave}
        >
          {isHovered && !isAuthorSelf && (
            <UserHoverPopover
              followStatus={followStatus}
              loadingStatus={loadingStatus}
              followActionPending={followActionPending}
              onToggle={handleToggleFollow}
            />
          )}

          <UserContextMenu target={authorTarget}>
            <Link
              to={profilePath(note.user.username, note.user.domain)}
              className={styles.avatarLink}
              onClick={(e) => e.stopPropagation()}
            >
              <Avatar
                url={note.user.avatarUrl}
                name={note.user.displayName || note.user.username}
                size={large ? 48 : small ? 32 : 40}
              />
            </Link>
          </UserContextMenu>
          <span className={styles.names}>
            <span className={styles.nameRow}>
              <UserContextMenu target={authorTarget}>
                <Link
                  to={profilePath(note.user.username, note.user.domain)}
                  className={styles.displayNameLink}
                  onClick={(e) => e.stopPropagation()}
                >
                  <span className={styles.displayName}>
                    <EmojiText text={displayName(note)} emojis={note.emojis} />
                  </span>
                </Link>
              </UserContextMenu>
              {linkToDetail ? (
                <Link
                  to={`/notes/${note.id}`}
                  className={styles.time}
                  onClick={(e) => e.stopPropagation()}
                >
                  <time>{formatDate(note.createdAt)}</time>
                </Link>
              ) : (
                <time className={styles.time}>{formatDate(note.createdAt)}</time>
              )}
            </span>
            <span className={styles.acctRow}>
              <UserContextMenu target={authorTarget}>
                <Link
                  to={profilePath(note.user.username, note.user.domain)}
                  className={styles.acctLink}
                  onClick={(e) => e.stopPropagation()}
                >
                  <span className={styles.acct}>
                    {acct(note)}
                    {badge && note.user.actorType === "remote_seiran" && (
                      <span className={styles.protoBadge} title={badge.label}>
                        {badge.iconUrl ? (
                          <img
                            src={badge.iconUrl}
                            alt={badge.icon}
                            className={styles.protoSeiranIcon}
                          />
                        ) : (
                          <TwemojiEmoji emoji={badge.icon} />
                        )}
                      </span>
                    )}
                    {delBadges.map((b) => (
                      <span
                        key={b.protocol}
                        className={styles.protoBadge}
                        title={b.label}
                      >
                        {b.protocol === "bsky" ? (
                          <img src={blueskyLogo} alt="" className={styles.deliveryBskyIcon} />
                        ) : (
                          <img src={fediverseLogo} alt="" className={styles.deliveryFediIcon} />
                        )}
                      </span>
                    ))}
                    {visBadge && (
                      <span className={styles.protoBadge} title={visBadge.label}>
                        <TwemojiEmoji emoji={visBadge.icon} />
                      </span>
                    )}
                  </span>
                </Link>
              </UserContextMenu>
              {remoteInfo && (
                <span
                  className={styles.remoteServerBadge}
                  style={{ background: remoteInfo.bg }}
                  title={remoteInfo.label}
                >
                  {remoteInfo.useBlueskyLogo ? (
                    <img src={blueskyLogo} alt="" className={styles.remoteServerIcon} />
                  ) : (
                    remoteInfo.iconUrl && (
                      <img src={mediaUrl(remoteInfo.iconUrl)} alt="" className={styles.remoteServerIcon} />
                    )
                  )}
                  <span className={styles.remoteServerLabel}>{remoteInfo.label}</span>
                </span>
              )}
            </span>
          </span>
        </div>
      </div>

      {(effectiveReplyId || note.replyStatus || note.quoteId) && (
        <div className={styles.relations}>
          {effectiveReplyId ? (
            <ReplyIndicator replyId={effectiveReplyId} onClimb={onReplyIndicatorClick} />
          ) : (
            note.replyStatus && (
              <PendingReferenceIndicator
                noteId={note.id}
                kind="reply"
                status={note.replyStatus}
                onResolved={setResolvedReplyId}
              />
            )
          )}
          {note.quoteId && (
            <Link
              to={`/notes/${note.quoteId}`}
              className={styles.relLink}
              onClick={(e) => e.stopPropagation()}
            >
              {t("home:noteCard.quoteSourceLink")}
            </Link>
          )}
        </div>
      )}

      {note.contentWarning && (
        <div className={styles.contentWarningWrap}>
          <p className={styles.contentWarningText}>
            <TwemojiEmoji emoji="⚠️" /> <EmojiText text={note.contentWarning} emojis={note.emojis} />
          </p>
          <button
            className={styles.contentWarningToggle}
            onClick={(e) => {
              e.stopPropagation();
              setShowContent((shown) => !shown);
            }}
          >
            {showContent
              ? t("home:noteCard.hideContent")
              : t("home:noteCard.showContent")}
          </button>
        </div>
      )}
      {showContent && (
        <>
          <p className={styles.body}>
            {note.contentHtml ? (
              <RichHtml html={note.contentHtml} emojis={note.emojis} />
            ) : (
              <RichText text={note.text} emojis={note.emojis} />
            )}
          </p>
          <NoteAttachments attachments={note.attachments} />
          {note.linkCards.map((card) => (
            <LinkCard key={card.url} card={card} />
          ))}
        </>
      )}

      {showContent && poll && (
        <div className={styles.poll}>
          {pollResults || pollVoted || pollClosed
            ? poll.options.map((option, index) => (
                <div
                  className={`${styles.pollOption} ${sharedPollState?.votedByMe.includes(index) ? styles.pollOptionVoted : ""}`}
                  key={option.name}
                >
                  <span>
                    {sharedPollState?.votedByMe.includes(index) && "✓ "}
                    {option.name}
                  </span>
                  <span>
                    {t("home:noteCard.votes", { count: option.votes })}
                  </span>
                </div>
              ))
            : poll.options.map((option, index) =>
                poll.multiple ? (
                  <label className={styles.pollChoice} key={option.name}>
                    <input
                      type="checkbox"
                      checked={pollSelection.includes(index)}
                      disabled={pollPending}
                      onChange={(e) =>
                        setPollSelection((selected) =>
                          e.target.checked
                            ? [...selected, index]
                            : selected.filter((i) => i !== index),
                        )
                      }
                    />
                    <span>{option.name}</span>
                  </label>
                ) : (
                  <button
                    className={styles.pollChoice}
                    key={option.name}
                    disabled={pollPending}
                    onClick={(e) => {
                      e.stopPropagation();
                      void submitPollVote([index]);
                    }}
                  >
                    {option.name}
                  </button>
                ),
              )}
          {pollClosed && (
            <div className={styles.pollControls}>
              <span className={styles.pollClosedLabel}>
                {t("home:noteCard.pollClosed")}
              </span>
            </div>
          )}
          {!pollVoted && !pollClosed && (
            <div className={styles.pollControls}>
              {poll.multiple && !pollResults && (
                <button
                  disabled={pollPending || pollSelection.length === 0}
                  onClick={(e) => {
                    e.stopPropagation();
                    void submitPollVote(pollSelection);
                  }}
                >
                  {t("home:noteCard.pollVoteButton")}
                </button>
              )}
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  setPollResults((shown) => !shown);
                }}
              >
                {pollResults
                  ? t("home:noteCard.pollBackToOptions")
                  : t("home:noteCard.pollShowResults")}
              </button>
              {poll.endTime && (
                <PollCountdown endTime={poll.endTime} className={styles.pollRemaining} />
              )}
            </div>
          )}
        </div>
      )}

      {showContent && (note.quote || resolvedQuote) && (
        <QuoteCard note={resolvedQuote ?? note.quote!} />
      )}
      {showContent && !note.quote && !resolvedQuote && note.quoteStatus && (
        <div className={styles.pendingQuoteWrap}>
          <PendingReferenceIndicator
            noteId={note.id}
            kind="quote"
            status={note.quoteStatus}
            onResolved={handleQuoteResolved}
          />
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
        subjectLabel={t("home:noteCard.reportSubject", {
          username: note.user.username,
        })}
        replyCount={note.replyCount}
        quoteCount={note.quoteCount}
        repostCount={note.repostCount}
        reactions={reactions}
        reactionPending={reactionPending}
        onToggleReaction={toggleReaction}
        onReply={handleReply}
        onQuote={handleQuote}
        isPrivateQuoteTarget={isPrivateQuoteTarget}
        isGateReplyBlocked={isGateReplyBlocked}
        isGateQuoteBlocked={isGateQuoteBlocked}
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
        indent={!large}
      />
    </>
  );
}

/**
 * リポストラッパーの先頭行（🔁 + リポストした人 + リポスト日時へのリンク）。
 * 元投稿の取り込み状況（正常/非公開/pending/gone）によらず、
 * リポストラッパー自身（note.id）の詳細ページへは常に遷移できる必要があるため、
 * renote/renoteId/renoteStatusの各分岐で共通利用する。
 */
function RepostRail({ note }: { note: Note }) {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const suffix = t("home:noteCard.repostedSuffix");
  const targetKey = profileQuery(note.user.username, note.user.domain);
  const isSelf =
    !!currentUser &&
    currentUser.username === note.user.username &&
    (!note.user.domain || note.user.domain === window.location.hostname);

  // note.user はリポストした人（PostContent側の著者とは別人）。タイムラインAPI
  // レスポンスに事前付与された関係情報をストア未登録時のみシードする（NoteCard本体の
  // シード処理と同じ理由、stores/userRelationshipStore.ts の seedRelationshipIfAbsent 参照）。
  useEffect(() => {
    if (isSelf || note.user.followStatus === undefined) return;
    seedRelationshipIfAbsent(targetKey, {
      followStatus: note.user.followStatus,
      isMuted: note.user.isMuted ?? false,
      isBlocking: note.user.isBlocking ?? false,
      isBlockedBy: note.user.isBlockedBy ?? false,
      isRepostMuted: note.user.isRepostMuted ?? false,
    });
  }, [
    targetKey,
    isSelf,
    note.user.followStatus,
    note.user.isMuted,
    note.user.isBlocking,
    note.user.isBlockedBy,
    note.user.isRepostMuted,
  ]);

  const repostTarget: UserRelationshipTarget = {
    username: note.user.username,
    domain: note.user.domain,
    actorId: String(note.user.id),
    reportLabel: `@${note.user.username}${note.user.domain ? `@${note.user.domain}` : ""}`,
  };

  return (
    <div className={styles.rail}>
      <TwemojiEmoji emoji="🔁" />{" "}
      <UserContextMenu target={repostTarget}>
        <strong>
          <EmojiText text={displayName(note)} emojis={note.emojis} />
        </strong>
      </UserContextMenu>{" "}
      {t("home:noteCard.repostedConnector")}{" "}
      <Link
        to={`/notes/${note.id}`}
        className={styles.repostTime}
        onClick={(e) => e.stopPropagation()}
      >
        {formatDate(note.createdAt)}
      </Link>
      {suffix && <> {suffix}</>}
    </div>
  );
}

export default function NoteCard({
  note,
  linkToDetail = true,
  large = false,
  small = false,
  forceOpenCw = false,
  onReplyIndicatorClick,
}: NoteCardProps) {
  const { t } = useTranslation();
  const [hidden, setHidden] = useState(false);
  // pendingなリポスト対象が「取り込む」で解決された場合のローカル反映（#234）。
  const [resolvedRenote, setResolvedRenote] = useState<Note | null>(null);
  const sizeClass = large ? styles.large : small ? styles.small : "";

  if (hidden) return null;

  async function handleRenoteResolved(resolvedId: string) {
    try {
      setResolvedRenote(await api.notes.get(resolvedId));
    } catch {
      // 取得に失敗しても致命的ではない（対象IDは解決済みのため次回リロードでは表示される）
    }
  }

  const effectiveRenote = note.renote ?? resolvedRenote;
  if (effectiveRenote) {
    return (
      <article className={`${styles.card} ${sizeClass}`}>
        <RepostRail note={note} />
        <PostContent
          note={effectiveRenote}
          // 元投稿は常にリポストラッパー自身(note)とは別ページのため、
          // 親から渡されたlinkToDetail（詳細ページ自身が自分自身へのリンクを消すためのフラグ）
          // を伝播させず、常にリンクを有効にする（元投稿の日付が無反応だった不具合の修正）。
          linkToDetail
          large={large}
          small={small}
          onUnreposted={() => setHidden(true)}
          onDeleted={() => setHidden(true)}
          forceOpenCw={forceOpenCw}
          onReplyIndicatorClick={onReplyIndicatorClick}
        />
      </article>
    );
  }

  // renoteId はあるが renote が欠落している場合、元ポストが非公開（プライベート/ひかえめ）で
  // 閲覧者から見えないケース（embed_renotes の可視性ガードによるもの）。
  if (note.renoteId) {
    return (
      <article className={`${styles.card} ${sizeClass}`}>
        <RepostRail note={note} />
        <p className={styles.unavailableNote}>
          {t("home:noteCard.unavailableRepost")}
        </p>
      </article>
    );
  }

  // 対象が見当たらないが誰かが何かをリポストしたこと自体は分かるケース
  // （取り込み時にリポスト対象のフェッチに失敗した、#230〜#232）。
  if (note.renoteStatus) {
    return (
      <article className={`${styles.card} ${sizeClass}`}>
        <RepostRail note={note} />
        <div className={styles.pendingQuoteWrap}>
          <PendingReferenceIndicator
            noteId={note.id}
            kind="repost"
            status={note.renoteStatus}
            onResolved={handleRenoteResolved}
          />
        </div>
      </article>
    );
  }

  return (
    <article className={`${styles.card} ${sizeClass}`}>
      <PostContent
        note={note}
        linkToDetail={linkToDetail}
        large={large}
        small={small}
        onDeleted={() => setHidden(true)}
        forceOpenCw={forceOpenCw}
        onReplyIndicatorClick={onReplyIndicatorClick}
      />
    </article>
  );
}
