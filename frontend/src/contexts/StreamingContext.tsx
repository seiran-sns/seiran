import { createContext, useCallback, useContext, useRef, useState } from "react";
import { Note, noteFromStream, ReactionSummary } from "../api/client";
import { useAuth } from "./AuthContext";
import { useStreaming } from "../hooks/useStreaming";

/** 通知（リアクション/フォロー/フォロー承諾）1件。 */
export interface Notif {
  id: number;
  kind: "reaction" | "follow" | "followAccepted";
  body: {
    postId?: string;
    emoji?: string;
    actor?: { username?: string; domain?: string; displayName?: string };
  };
  at: number;
}

type NoteListener = (n: Note) => void;

/** `noteUpdated`（リアクション追加/切替/取消）のライブ更新1件。 */
export interface ReactionUpdate {
  postId: string;
  /** 更新後の権威的な集計（`reactedByMe` は含まない。閲覧者ごとに異なるため）。 */
  reactions: Pick<ReactionSummary, "emoji" | "count">[];
  /** 操作した本人の actor_id。自分自身の別タブ/端末からの操作かの判定に使う。 */
  reactorActorId: number;
  /** 操作後、reactor がこの投稿に付けているリアクション（切替/追加なら絵文字、取消なら null）。 */
  reactorEmoji: string | null;
}

type ReactionListener = (u: ReactionUpdate) => void;

interface StreamingValue {
  notifications: Notif[];
  unread: number;
  markRead: () => void;
  /** 新規ポスト受信リスナーを登録する（HomePage が TL 先頭挿入に使用）。戻り値で解除。 */
  registerNote: (cb: NoteListener) => () => void;
  /** 指定ノートIDのリアクションのライブ更新を購読する（NoteCard が使用）。戻り値で解除。 */
  registerReaction: (noteId: string, cb: ReactionListener) => () => void;
}

const StreamingContext = createContext<StreamingValue>({
  notifications: [],
  unread: 0,
  markRead: () => {},
  registerNote: () => () => {},
  registerReaction: () => () => {},
});

const NOTIF_KINDS = new Set(["reaction", "follow", "followAccepted"]);
let notifSeq = 0;

export function StreamingProvider({ children }: { children: React.ReactNode }) {
  const { user } = useAuth();
  const [notifications, setNotifications] = useState<Notif[]>([]);
  const [unread, setUnread] = useState(0);
  const noteListeners = useRef<Set<NoteListener>>(new Set());
  const reactionListeners = useRef<Map<string, Set<ReactionListener>>>(new Map());

  useStreaming((type, body) => {
    if (type === "note") {
      const n = noteFromStream(body);
      noteListeners.current.forEach((cb) => cb(n));
    } else if (type === "noteUpdated") {
      const update = body as ReactionUpdate;
      reactionListeners.current.get(update.postId)?.forEach((cb) => cb(update));
    } else if (NOTIF_KINDS.has(type)) {
      const notif: Notif = { id: ++notifSeq, kind: type as Notif["kind"], body: (body ?? {}) as Notif["body"], at: Date.now() };
      setNotifications((prev) => [notif, ...prev].slice(0, 100));
      setUnread((u) => u + 1);
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

  const markRead = useCallback(() => setUnread(0), []);

  return (
    <StreamingContext.Provider value={{ notifications, unread, markRead, registerNote, registerReaction }}>
      {children}
    </StreamingContext.Provider>
  );
}

export function useStreamingContext() {
  return useContext(StreamingContext);
}
