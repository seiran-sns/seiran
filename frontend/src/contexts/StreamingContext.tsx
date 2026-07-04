import { createContext, useCallback, useContext, useRef, useState } from "react";
import { Note, noteFromStream } from "../api/client";
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

interface StreamingValue {
  notifications: Notif[];
  unread: number;
  markRead: () => void;
  /** 新規ポスト受信リスナーを登録する（HomePage が TL 先頭挿入に使用）。戻り値で解除。 */
  registerNote: (cb: NoteListener) => () => void;
}

const StreamingContext = createContext<StreamingValue>({
  notifications: [],
  unread: 0,
  markRead: () => {},
  registerNote: () => () => {},
});

const NOTIF_KINDS = new Set(["reaction", "follow", "followAccepted"]);
let notifSeq = 0;

export function StreamingProvider({ children }: { children: React.ReactNode }) {
  const { user } = useAuth();
  const [notifications, setNotifications] = useState<Notif[]>([]);
  const [unread, setUnread] = useState(0);
  const noteListeners = useRef<Set<NoteListener>>(new Set());

  useStreaming((type, body) => {
    if (type === "note") {
      const n = noteFromStream(body);
      noteListeners.current.forEach((cb) => cb(n));
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

  const markRead = useCallback(() => setUnread(0), []);

  return (
    <StreamingContext.Provider value={{ notifications, unread, markRead, registerNote }}>
      {children}
    </StreamingContext.Provider>
  );
}

export function useStreamingContext() {
  return useContext(StreamingContext);
}
