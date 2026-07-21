import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { api, Note, noteFromStream, ReactionSummary } from "../api/client";
import { useAuth } from "./AuthContext";
import { useStreaming } from "../hooks/useStreaming";

type NoteListener = (n: Note) => void;

/** `noteUpdated`（リアクション追加/切替/取消）のライブ更新1件。 */
export interface ReactionUpdate {
  postId: string;
  /** 更新後の権威的な集計（`reactedByMe` は含まない。閲覧者ごとに異なるため）。 */
  reactions: Pick<ReactionSummary, "emoji" | "count" | "emojiUrl">[];
  /** 操作した本人の actor_id。自分自身の別タブ/端末からの操作かの判定に使う。 */
  reactorActorId: number;
  /** 操作後、reactor がこの投稿に付けているリアクション（切替/追加なら絵文字、取消なら null）。 */
  reactorEmoji: string | null;
}

type ReactionListener = (u: ReactionUpdate) => void;

/**
 * 通知系イベント（reaction/follow/followAccepted）が届いたことのみを知らせるリスナー。
 * ペイロードは使わない。通知の永続化（`POST /api/i/notifications`）に一本化したため、
 * WS ペイロードは「新着があったので再取得せよ」という即時性のためのシグナルに過ぎない。
 */
type NotifListener = () => void;

interface StreamingValue {
  unread: number;
  markRead: () => void;
  /** 新規ポスト受信リスナーを登録する（HomePage が TL 先頭挿入に使用）。戻り値で解除。 */
  registerNote: (cb: NoteListener) => () => void;
  /** 指定ノートIDのリアクションのライブ更新を購読する（NoteCard が使用）。戻り値で解除。 */
  registerReaction: (noteId: string, cb: ReactionListener) => () => void;
  /** 通知の新着シグナルを購読する（NotificationsPanel が使用）。戻り値で解除。 */
  registerNotifArrived: (cb: NotifListener) => () => void;
  /** DM新着（visibility="direct"のnoteイベント）を購読する（MessagesPageが使用）。戻り値で解除。 */
  registerDirectMessage: (cb: NoteListener) => () => void;
  /** 未読のあるDMセッション数（左ペインバッジ用）。 */
  dmUnreadCount: number;
  /** サーバーから未読数を再取得する（既読操作後・ページロード時に呼ぶ）。 */
  refreshDmUnreadCount: () => void;
}

const StreamingContext = createContext<StreamingValue>({
  unread: 0,
  markRead: () => {},
  registerNote: () => () => {},
  registerReaction: () => () => {},
  registerNotifArrived: () => () => {},
  registerDirectMessage: () => () => {},
  dmUnreadCount: 0,
  refreshDmUnreadCount: () => {},
});

const NOTIF_KINDS = new Set(["reaction", "follow", "followAccepted", "mention"]);

export function StreamingProvider({ children }: { children: React.ReactNode }) {
  const { user } = useAuth();
  const [unread, setUnread] = useState(0);
  const [dmUnreadCount, setDmUnreadCount] = useState(0);
  const noteListeners = useRef<Set<NoteListener>>(new Set());
  const reactionListeners = useRef<Map<string, Set<ReactionListener>>>(new Map());
  const notifListeners = useRef<Set<NotifListener>>(new Set());
  const dmListeners = useRef<Set<NoteListener>>(new Set());

  const refreshDmUnreadCount = useCallback(() => {
    if (!user) return;
    api.dm.unreadCount().then((r) => setDmUnreadCount(r.count)).catch(() => {});
  }, [user]);

  useEffect(() => {
    refreshDmUnreadCount();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [user?.id]);

  useStreaming((type, body) => {
    if (type === "note") {
      const n = noteFromStream(body);
      if (n.visibility === "direct") {
        dmListeners.current.forEach((cb) => cb(n));
        refreshDmUnreadCount();
      } else {
        noteListeners.current.forEach((cb) => cb(n));
      }
    } else if (type === "noteUpdated") {
      const update = body as ReactionUpdate;
      reactionListeners.current.get(update.postId)?.forEach((cb) => cb(update));
    } else if (NOTIF_KINDS.has(type)) {
      setUnread((u) => u + 1);
      notifListeners.current.forEach((cb) => cb());
    }
  }, user?.id ?? null);

  const registerNote = useCallback((cb: NoteListener) => {
    noteListeners.current.add(cb);
    return () => {
      noteListeners.current.delete(cb);
    };
  }, []);

  const registerReaction = useCallback((noteId: string, cb: ReactionListener) => {
    let set = reactionListeners.current.get(noteId);
    if (!set) {
      set = new Set();
      reactionListeners.current.set(noteId, set);
    }
    set.add(cb);
    return () => {
      const s = reactionListeners.current.get(noteId);
      if (!s) return;
      s.delete(cb);
      if (s.size === 0) reactionListeners.current.delete(noteId);
    };
  }, []);

  const registerNotifArrived = useCallback((cb: NotifListener) => {
    notifListeners.current.add(cb);
    return () => {
      notifListeners.current.delete(cb);
    };
  }, []);

  const registerDirectMessage = useCallback((cb: NoteListener) => {
    dmListeners.current.add(cb);
    return () => {
      dmListeners.current.delete(cb);
    };
  }, []);

  const markRead = useCallback(() => setUnread(0), []);

  return (
    <StreamingContext.Provider
      value={{
        unread,
        markRead,
        registerNote,
        registerReaction,
        registerNotifArrived,
        registerDirectMessage,
        dmUnreadCount,
        refreshDmUnreadCount,
      }}
    >
      {children}
    </StreamingContext.Provider>
  );
}

export function useStreamingContext() {
  return useContext(StreamingContext);
}
