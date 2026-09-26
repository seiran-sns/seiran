import { FormEvent, lazy, Suspense, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, DmSession, getErrorMessage, Note } from "../api/client";
import AppShell from "../components/layout/AppShell";
import Avatar from "../components/note/Avatar";
import EmojiText from "../components/note/EmojiText";
import MessageContent from "../components/note/MessageContent";
import MessageContextMenu from "../components/note/MessageContextMenu";
import MessageReactions from "../components/note/MessageReactions";
import RecipientPicker, { RecipientChip } from "../components/dm/RecipientPicker";
import { useAuth } from "../contexts/AuthContext";
import { useStreamingContext } from "../contexts/StreamingContext";
import { useToast } from "../contexts/ToastContext";
import { applyReactionUpdate } from "../hooks/useNoteCardActions";
import Modal from "../components/common/Modal";
import TwemojiEmoji from "../components/common/TwemojiEmoji";
import styles from "./MessagesPage.module.css";

// Unicode 絵文字データセットを含むため、ピッカーを実際に開くまでロードしない
// （`ReactionPicker`と同じバンドルサイズ対策）。
const EmojiPickerPanel = lazy(() => import("../components/note/EmojiPickerPanel"));

/** バックエンドの上限と対応（`validate_dm_text_length`）。 */
const BSKY_DM_MAX = 1000;
const FEDI_DM_MAX = 3000;

function peerLabel(session: DmSession, t: (key: string) => string): string {
  if (session.peers.length === 0) return t("dm:messagesPage.unknownPeer");
  return session.peers.map((p) => p.displayName || p.username).join(", ");
}

export default function MessagesPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { threadRootId } = useParams<{ threadRootId?: string }>();
  const { user } = useAuth();
  const { registerDirectMessage, registerReaction, refreshDmUnreadCount } = useStreamingContext();
  const { showError } = useToast();

  const [sessions, setSessions] = useState<DmSession[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(true);
  const [messages, setMessages] = useState<Note[]>([]);
  const [messagesLoading, setMessagesLoading] = useState(false);
  const [recipients, setRecipients] = useState<RecipientChip[]>([]);
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  /** リアクションピッカーを開いているメッセージID。 */
  const [reactionPickerFor, setReactionPickerFor] = useState<string | null>(null);
  /** 削除確認モーダルの対象メッセージID。 */
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);

  function reloadSessions() {
    return api.dm.sessions({ limit: 50 }).then(setSessions);
  }

  useEffect(() => {
    setSessionsLoading(true);
    reloadSessions().finally(() => setSessionsLoading(false));
  }, []);

  useEffect(() => {
    if (!threadRootId) {
      setMessages([]);
      setRecipients([]);
      return;
    }
    let cancelled = false;
    setMessagesLoading(true);
    api.dm
      .threadMessages(threadRootId, { limit: 200 })
      .then((rows) => {
        if (!cancelled) setMessages(rows);
      })
      .finally(() => !cancelled && setMessagesLoading(false));
    api.dm
      .markRead(threadRootId)
      .then(() => {
        reloadSessions();
        refreshDmUnreadCount();
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadRootId]);

  // 選択中セッションの相手を宛先chipへ自動設定する（返信時は宛先固定）。
  useEffect(() => {
    if (!threadRootId) return;
    const session = sessions.find((s) => s.threadRootPostId === threadRootId);
    if (session) {
      setRecipients(
        session.peers.map((p) => ({
          actorId: p.id,
          username: p.username,
          domain: p.domain,
          displayName: p.displayName,
          actorType: p.actorType,
          avatarUrl: p.avatarUrl,
        }))
      );
    }
  }, [threadRootId, sessions]);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [messages]);

  useEffect(
    () =>
      registerDirectMessage(() => {
        reloadSessions();
        if (threadRootId) {
          api.dm.threadMessages(threadRootId, { limit: 200 }).then(setMessages);
        }
      }),
    [registerDirectMessage, threadRootId]
  );

  // 表示中の各メッセージへの絵文字リアクション追加/切替/取消をリアルタイム反映する
  // （通常投稿のNoteCardと同じ`noteUpdated`イベント・`applyReactionUpdate`を再利用。
  // fedi/local宛は`broadcast_reaction_update`、bsky宛は`bsky_dm_poll`のポーリング検知が
  // 送出元）。メッセージID一覧が変わったときのみ購読し直す（reactions自体の更新で
  // 配列参照が変わっても再購読しないよう、依存はID列のみのプリミティブ文字列にする）。
  const messageIds = messages.map((m) => m.id).join(",");
  useEffect(() => {
    if (!messageIds) return;
    const unsubs = messageIds.split(",").map((id) =>
      registerReaction(id, (update) => {
        setMessages((prev) =>
          prev.map((m) =>
            m.id === update.postId
              ? { ...m, reactions: applyReactionUpdate(m.reactions ?? [], update, user?.actor_id) }
              : m
          )
        );
      })
    );
    return () => unsubs.forEach((u) => u());
  }, [messageIds, registerReaction, user?.actor_id]);

  const hasBskyRecipient = recipients.some((r) => r.actorType === "bsky");
  const hasBskyIssue = hasBskyRecipient && recipients.length > 1;
  const maxLen = hasBskyRecipient ? BSKY_DM_MAX : FEDI_DM_MAX;
  const canShowAttachmentButton = !hasBskyRecipient;

  async function handleSend(e: FormEvent) {
    e.preventDefault();
    if (!text.trim() || recipients.length === 0 || sending || hasBskyIssue) return;
    setSending(true);
    setError("");
    try {
      const created = await api.notes.create(
        text,
        true,
        true,
        [],
        threadRootId,
        undefined,
        "direct",
        recipients.map((r) => r.actorId)
      );
      setText("");
      await reloadSessions();
      if (!threadRootId) {
        navigate(`/messages/${created.id}`);
      } else {
        // 送信直後の楽観的追加（配列へのpush）はWS経由の再取得（registerDirectMessage、
        // 全件を取り直すfull replace）と非同期に競合し、タイミング次第で同じメッセージが
        // 二重表示される回帰バグがあった。「楽観的追加」と「WS再取得」という2つの独立した
        // 経路がそれぞれ別々に画面状態を更新しようとするのがレースの根本原因のため、
        // 送信後もWSと同じ「サーバーから取り直して丸ごと置き換える」経路に一本化する
        // （full replace同士は順序によらず必ず同じ最終状態に収束し、重複が起こり得ない）。
        setMessages(await api.dm.threadMessages(threadRootId, { limit: 200 }));
      }
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSending(false);
    }
  }

  async function addReaction(messageId: string, emoji: string) {
    setReactionPickerFor(null);
    try {
      const result = hasBskyRecipient
        ? await api.dm.reactBsky(messageId, emoji)
        : await api.notes.react(messageId, emoji);
      setMessages((prev) =>
        prev.map((m) => (m.id === messageId ? { ...m, reactions: result.reactions } : m)),
      );
    } catch (err) {
      showError(getErrorMessage(err));
    }
  }

  async function confirmDeleteMessage() {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      if (hasBskyRecipient) {
        await api.dm.hideMessage(deleteTarget);
      } else {
        await api.notes.delete(deleteTarget);
      }
      setMessages((prev) => prev.filter((m) => m.id !== deleteTarget));
      setDeleteTarget(null);
    } catch (err) {
      showError(getErrorMessage(err));
    } finally {
      setDeleting(false);
    }
  }

  const right = (
    <>
      <Link className={styles.newButton} to="/messages">
        {t("dm:messagesPage.newMessage")}
      </Link>
      <ul className={styles.sessionList}>
        {sessionsLoading && <li className={styles.loading}>{t("common:loading")}</li>}
        {!sessionsLoading && sessions.length === 0 && (
          <li className={styles.empty}>{t("dm:messagesPage.emptySessions")}</li>
        )}
        {sessions.map((s) => (
          <li key={s.threadRootPostId}>
            <Link
              to={`/messages/${s.threadRootPostId}`}
              className={`${styles.sessionItem} ${s.threadRootPostId === threadRootId ? styles.sessionItemActive : ""}`}
            >
              <Avatar url={s.peers[0]?.avatarUrl} name={peerLabel(s, t)} size={36} />
              <span className={styles.sessionInfo}>
                <span className={styles.sessionName}>{peerLabel(s, t)}</span>
                <span className={styles.sessionPreview}>
                  {s.lastMessage.contentWarning ? (
                    <>
                      <TwemojiEmoji emoji="⚠️" />{" "}
                      <EmojiText text={s.lastMessage.contentWarning} emojis={s.lastMessage.emojis} />
                    </>
                  ) : (
                    <EmojiText text={s.lastMessage.text} emojis={s.lastMessage.emojis} />
                  )}
                </span>
              </span>
              {s.unread && <span className={styles.unreadDot} />}
            </Link>
          </li>
        ))}
      </ul>
    </>
  );

  const center = (
    <>
      <div className={styles.messageList} ref={scrollRef}>
        {messagesLoading && <p className={styles.loading}>{t("common:loading")}</p>}
        {!messagesLoading && !threadRootId && <p className={styles.empty}>{t("dm:messagesPage.composeHint")}</p>}
        {!messagesLoading &&
          messages.map((m) => {
            const isMine = m.user.id === user?.actor_id;
            // 3人以上（自分+宛先2人以上）が参加するスレッドでのみ、メッセージごとの宛先を表示する
            // （1対1では常に相手全員に届くため不要）。
            const showRecipients = recipients.length >= 2 && !!m.recipients?.length;
            return (
              <div key={m.id} className={`${styles.messageRow} ${isMine ? styles.messageRowMine : ""}`}>
                {!isMine && <Avatar url={m.user.avatarUrl} name={m.user.displayName || m.user.username} size={28} />}
                <div>
                  <MessageContextMenu
                    canDelete={isMine || hasBskyRecipient}
                    isBsky={hasBskyRecipient}
                    onReact={() => setReactionPickerFor(m.id)}
                    onDelete={() => setDeleteTarget(m.id)}
                  >
                    <div className={`${styles.messageBubble} ${isMine ? styles.messageBubbleMine : ""}`}>
                      {showRecipients && (
                        <div className={styles.messageRecipients}>
                          <span className={styles.messageRecipientsLabel}>{t("dm:messagesPage.toLabel")}</span>
                          {m.recipients!.map((r) => (
                            <span
                              key={r.id}
                              className={styles.messageRecipientAvatar}
                              title={`@${r.username}${r.domain ? `@${r.domain}` : ""}\n${r.displayName || r.username}`}
                            >
                              <Avatar url={r.avatarUrl} name={r.displayName || r.username} size={16} />
                            </span>
                          ))}
                        </div>
                      )}
                      <div className={styles.messageText}>
                        <MessageContent note={m} />
                      </div>
                      <span className={styles.messageTime}>{new Date(m.createdAt).toLocaleString()}</span>
                    </div>
                  </MessageContextMenu>
                  <MessageReactions reactions={m.reactions} />
                </div>
              </div>
            );
          })}
      </div>

      <form className={styles.composer} onSubmit={handleSend}>
        <RecipientPicker value={recipients} onChange={setRecipients} />
        {hasBskyIssue && <p className={styles.error}>{t("dm:messagesPage.bskySingleRecipientError")}</p>}
        <div className={styles.textRow}>
          {canShowAttachmentButton && (
            <button type="button" className={styles.attachButton} disabled title={t("dm:messagesPage.attachButtonTitle")}>
              <TwemojiEmoji emoji="📎" />
            </button>
          )}
          <textarea
            className={styles.textarea}
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={t("dm:messagesPage.textPlaceholder")}
            maxLength={maxLen}
            rows={2}
          />
          <button type="submit" disabled={sending || !text.trim() || recipients.length === 0 || hasBskyIssue}>
            {t("dm:messagesPage.sendButton")}
          </button>
        </div>
        <div className={styles.charCount}>
          {text.length}/{maxLen}
        </div>
        {error && <p className={styles.error}>{error}</p>}
      </form>
    </>
  );

  return (
    <>
      <AppShell center={center} right={right} />
      <Modal
        open={reactionPickerFor !== null}
        onClose={() => setReactionPickerFor(null)}
        title={t("home:reactionPicker.addReactionTitle")}
      >
        {reactionPickerFor !== null && (
          <Suspense fallback={<p>{t("common:loading")}</p>}>
            <EmojiPickerPanel
              onPick={(emoji) => addReaction(reactionPickerFor, emoji)}
              unicodeOnly={hasBskyRecipient}
            />
          </Suspense>
        )}
      </Modal>
      <Modal
        open={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
        title={t(
          hasBskyRecipient ? "dm:messagesPage.hideConfirmModal.title" : "dm:messagesPage.deleteConfirmModal.title"
        )}
      >
        <p>{t(hasBskyRecipient ? "dm:messagesPage.hideConfirmModal.body" : "dm:messagesPage.deleteConfirmModal.body")}</p>
        <div className={styles.modalActions}>
          <button
            type="button"
            className={styles.modalPrimaryDanger}
            onClick={confirmDeleteMessage}
            disabled={deleting}
          >
            {t(
              hasBskyRecipient
                ? "dm:messagesPage.hideConfirmModal.confirmButton"
                : "dm:messagesPage.deleteConfirmModal.confirmButton"
            )}
          </button>
          <button type="button" className={styles.modalSecondary} onClick={() => setDeleteTarget(null)}>
            {t("common:cancel")}
          </button>
        </div>
      </Modal>
    </>
  );
}
