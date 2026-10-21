import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError, getErrorMessage, Note, ReactionSummary } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import { useToast } from "../contexts/ToastContext";
import { ReactionUpdate, useStreamingContext } from "../contexts/StreamingContext";
import { getReactionState, setReactionState, useReactionState } from "../stores/reactionStore";
import { useComposer } from "../contexts/ComposerContext";
import {
  clearComposerDraft,
  loadComposerDraft,
  saveComposerDraft,
} from "../lib/composerDraft";
import {
  buildRedraftDraft,
  RedraftBlockReason,
  redraftBlockReason,
  redraftDraftTarget,
} from "../lib/redraft";

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
  myActorId: string | undefined
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

/**
 * NoteCard（PostContent）が必要とするミューテーション（リアクション送信/取消・リポスト/
 * リポスト取消・ピン留め/解除）と、それに紐づくローカル state・リアルタイム反映をまとめて
 * 提供するフック。呼び出し側（PostContent）は返された state とハンドラを JSX に配線する
 * だけでよく、表示ロジックに専念できる。
 *
 * `onUnreposted` はリポスト取消が成功した際に呼ばれる（NoteCard 側でリポスト表示自体を
 * 非表示にするために使う）。`onDeleted` は投稿削除が成功した際に呼ばれる（同様にカード自体を
 * 非表示にするために使う）。
 */
export function useNoteCardActions(note: Note, onUnreposted?: () => void, onDeleted?: () => void) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const { showError } = useToast();
  const { registerReaction } = useStreamingContext();
  const { openReply, openQuote, openCompose } = useComposer();

  const [reposting, setReposting] = useState(false);
  const [unreposting, setUnreposting] = useState(false);
  const [reposted, setReposted] = useState(note.repostedByMe ?? false);
  const reactions = useReactionState(note.id, note.reactions);
  const [pinned, setPinned] = useState(note.pinnedByMe ?? false);
  const [pinning, setPinning] = useState(false);
  const [deleting, setDeleting] = useState(false);
  // 1投稿につき1ユーザー1リアクションまでのため、切り替え中は他の絵文字操作も
  // まとめてロックする（個別絵文字ごとの pending 管理はしない）。
  const [reactionPending, setReactionPending] = useState(false);

  const isSelf = note.user.actorType === "local" && !!user && user.username === note.user.username;
  const isPrivateRepostTarget = note.visibility === "followers_only" || note.visibility === "direct";
  const isPrivateQuoteTarget = note.visibility === "followers_only" || note.visibility === "direct";
  // Bsky postgate（引用可否）による制限。可視性由来の isPrivateQuoteTarget とは理由が異なる
  // （ツールチップ・トースト文言を分けるため別フラグで持つ）。
  const isGateQuoteBlocked = note.quoteBlocked ?? false;
  const isGateReplyBlocked = note.replyBlocked ?? false;

  // 他ユーザー（または自分の別タブ/端末）によるリアクション追加/切替/取消をリアルタイム反映する。
  useEffect(() => {
    return registerReaction(note.id, (update) => {
      setReactionState(note.id, applyReactionUpdate(getReactionState(note.id) ?? [], update, user?.actor_id));
    });
  }, [note.id, registerReaction, user?.actor_id]);

  async function handleRepost(e?: React.MouseEvent) {
    e?.stopPropagation();
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

    if (isPrivateRepostTarget || user?.did_moved_out) return;

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
    if (reacting && user?.did_moved_out) return;
    const prevReactions = reactions;

    setReactionPending(true);
    setReactionState(note.id, optimisticSetReaction(prevReactions, emoji, reacting));
    try {
      const res = reacting
        ? await api.notes.react(note.id, emoji)
        : await api.notes.unreact(note.id, emoji);
      setReactionState(note.id, res.reactions);
    } catch (err) {
      setReactionState(note.id, prevReactions);
      showError(getErrorMessage(err));
    } finally {
      setReactionPending(false);
    }
  }

  async function handleTogglePin(e?: React.MouseEvent) {
    e?.stopPropagation();
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

  async function handleDelete() {
    if (deleting) return;
    setDeleting(true);
    try {
      await api.notes.delete(note.id);
      onDeleted?.();
    } catch (err) {
      showError(getErrorMessage(err));
    } finally {
      setDeleting(false);
    }
  }

  /** 削除すると、再投稿先の下書き（作成中の内容）が置き換わるか。 */
  function redraftOverwritesDraft() {
    if (!user) return false;
    const existing = loadComposerDraft(redraftDraftTarget(note, user.id));
    return !!existing && existing.text.trim() !== "";
  }

  // 返信元・引用元が今も存在するか。`replyStatus`/`quoteStatus`は取り込み時の状態で、
  // 後から削除された場合は変わらないため、メニューを開くたびに取り直す。
  const [targetCheck, setTargetCheck] = useState<"idle" | "checking" | "ok" | "unavailable">("idle");
  const targetCheckRunning = useRef(false);

  function fetchRedraftTargets() {
    return Promise.all([
      note.replyId ? api.notes.get(note.replyId) : undefined,
      note.quoteId ? api.notes.get(note.quoteId) : undefined,
    ]);
  }

  function isTargetGone(err: unknown) {
    return err instanceof ApiError && (err.status === 404 || err.status === 403);
  }

  function checkRedraftTargets() {
    if ((!note.replyId && !note.quoteId) || targetCheckRunning.current) return;
    targetCheckRunning.current = true;
    setTargetCheck("checking");
    fetchRedraftTargets()
      .then(() => setTargetCheck("ok"))
      // 通信の失敗は存在しないとは限らないので不活性にしない（実行時にも確認し直す）。
      .catch((err) => setTargetCheck(isTargetGone(err) ? "unavailable" : "idle"))
      .finally(() => {
        targetCheckRunning.current = false;
      });
  }

  const staticBlock = redraftBlockReason(note);
  const redraftBlock: RedraftBlockReason | "checking" | null =
    staticBlock ??
    (targetCheck === "checking"
      ? "checking"
      : targetCheck === "unavailable"
        ? note.replyId
          ? "replyUnavailable"
          : "quoteUnavailable"
        : null);

  /**
   * 「削除して編集再投稿」: ポストの内容を下書きへ保存してから削除し、その下書きでコンポーザを開く。
   * 削除を先に確定させるのは、編集中にリプライやリアクションが付き続けるのを防ぐため。
   * 返信元・引用元は削除前に取得し、取れなければ何も消さずに中止する。下書きを削除より前に
   * 保存するのは、削除後にタブが閉じても内容を失わないため。
   */
  async function handleRedraft() {
    if (deleting || !user || staticBlock) return;
    setDeleting(true);
    const target = redraftDraftTarget(note, user.id);
    const previous = loadComposerDraft(target);
    try {
      const [replyTarget, quoteTarget] = await fetchRedraftTargets().catch((err) => {
        if (!isTargetGone(err)) throw err;
        setTargetCheck("unavailable");
        throw new Error(
          t(`home:noteCard.redraftBlocked.${note.replyId ? "replyUnavailable" : "quoteUnavailable"}`),
        );
      });
      saveComposerDraft(target, buildRedraftDraft(note));
      try {
        await api.notes.delete(note.id);
      } catch (err) {
        if (previous) saveComposerDraft(target, previous);
        else clearComposerDraft(target);
        throw err;
      }
      onDeleted?.();
      if (replyTarget) openReply(replyTarget);
      else if (quoteTarget) openQuote(quoteTarget);
      else openCompose();
    } catch (err) {
      showError(getErrorMessage(err));
    } finally {
      setDeleting(false);
    }
  }

  return {
    isSelf,
    isPrivateRepostTarget,
    isPrivateQuoteTarget,
    isGateQuoteBlocked,
    isGateReplyBlocked,
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
    handleRedraft,
    redraftOverwritesDraft,
    redraftBlock,
    checkRedraftTargets,
  };
}
