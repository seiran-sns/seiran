import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError, getErrorMessage, Note, ReactionSummary } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import { useToast } from "../contexts/ToastContext";
import { ReactionUpdate, useStreamingContext } from "../contexts/StreamingContext";

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

/**
 * NoteCard（PostContent）が必要とするミューテーション（リアクション送信/取消・リポスト/
 * リポスト取消・ピン留め/解除）と、それに紐づくローカル state・リアルタイム反映をまとめて
 * 提供するフック。呼び出し側（PostContent）は返された state とハンドラを JSX に配線する
 * だけでよく、表示ロジックに専念できる。
 *
 * `onUnreposted` はリポスト取消が成功した際に呼ばれる（NoteCard 側でリポスト表示自体を
 * 非表示にするために使う）。
 */
export function useNoteCardActions(note: Note, onUnreposted?: () => void) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const { showError } = useToast();
  const { registerReaction } = useStreamingContext();

  const [reposting, setReposting] = useState(false);
  const [unreposting, setUnreposting] = useState(false);
  const [reposted, setReposted] = useState(note.repostedByMe ?? false);
  const [reactions, setReactions] = useState<ReactionSummary[]>(note.reactions ?? []);
  const [pinned, setPinned] = useState(note.pinnedByMe ?? false);
  const [pinning, setPinning] = useState(false);
  // 1投稿につき1ユーザー1リアクションまでのため、切り替え中は他の絵文字操作も
  // まとめてロックする（個別絵文字ごとの pending 管理はしない）。
  const [reactionPending, setReactionPending] = useState(false);

  const isSelf = note.user.actorType === "local" && !!user && user.username === note.user.username;
  const isPrivateRepostTarget = note.visibility === "followers_only" || note.visibility === "direct";

  // 他ユーザー（または自分の別タブ/端末）によるリアクション追加/切替/取消をリアルタイム反映する。
  useEffect(() => {
    return registerReaction(note.id, (update) => {
      setReactions((prev) => applyReactionUpdate(prev, update, user?.actor_id));
    });
  }, [note.id, registerReaction, user?.actor_id]);

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

  return {
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
  };
}
