import { FormEvent, useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, DmSession, getErrorMessage, Note } from "../api/client";
import AppShell from "../components/layout/AppShell";
import Avatar from "../components/note/Avatar";
import RecipientPicker, { RecipientChip } from "../components/dm/RecipientPicker";
import { useAuth } from "../contexts/AuthContext";
import { useStreamingContext } from "../contexts/StreamingContext";
import panel from "../components/common/Panel.module.css";
import styles from "./MessagesPage.module.css";

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
  const { registerDirectMessage, refreshDmUnreadCount } = useStreamingContext();

  const [sessions, setSessions] = useState<DmSession[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(true);
  const [messages, setMessages] = useState<Note[]>([]);
  const [messagesLoading, setMessagesLoading] = useState(false);
  const [recipients, setRecipients] = useState<RecipientChip[]>([]);
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);

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

  const right = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>{t("dm:messagesPage.title")}</span>
        <button className={styles.newButton} onClick={() => navigate("/messages")}>
          {t("dm:messagesPage.newMessage")}
        </button>
      </header>
      <ul className={styles.sessionList}>
        {sessionsLoading && <li className={styles.loading}>{t("common:loading")}</li>}
        {!sessionsLoading && sessions.length === 0 && (
          <li className={styles.empty}>{t("dm:messagesPage.emptySessions")}</li>
        )}
        {sessions.map((s) => (
          <li key={s.threadRootPostId}>
            <button
              className={`${styles.sessionItem} ${s.threadRootPostId === threadRootId ? styles.sessionItemActive : ""}`}
              onClick={() => navigate(`/messages/${s.threadRootPostId}`)}
            >
              <Avatar url={s.peers[0]?.avatarUrl} name={peerLabel(s, t)} size={36} />
              <span className={styles.sessionInfo}>
                <span className={styles.sessionName}>{peerLabel(s, t)}</span>
                <span className={styles.sessionPreview}>{s.lastMessage.text}</span>
              </span>
              {s.unread && <span className={styles.unreadDot} />}
            </button>
          </li>
        ))}
      </ul>
    </>
  );

  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>
          {threadRootId && recipients.length > 0
            ? recipients.map((r) => r.displayName || r.username).join(", ")
            : t("dm:messagesPage.newMessage")}
        </span>
      </header>

      <div className={styles.messageList} ref={scrollRef}>
        {messagesLoading && <p className={styles.loading}>{t("common:loading")}</p>}
        {!messagesLoading && !threadRootId && <p className={styles.empty}>{t("dm:messagesPage.composeHint")}</p>}
        {!messagesLoading &&
          messages.map((m) => {
            const isMine = m.user.id === user?.actor_id;
            return (
              <div key={m.id} className={`${styles.messageRow} ${isMine ? styles.messageRowMine : ""}`}>
                {!isMine && <Avatar url={m.user.avatarUrl} name={m.user.displayName || m.user.username} size={28} />}
                <div className={`${styles.messageBubble} ${isMine ? styles.messageBubbleMine : ""}`}>
                  <p className={styles.messageText}>{m.text}</p>
                  <span className={styles.messageTime}>{new Date(m.createdAt).toLocaleString()}</span>
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
              📎
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

  return <AppShell center={center} right={right} />;
}
