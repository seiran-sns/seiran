import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { api, Note, ReactionSummary } from "../api/client";
import { profileQuery } from "../lib/format";
import { setFollowStatus as setFollowStatusStore } from "../stores/followStatusStore";
import { updatePollResults } from "../stores/pollVoteStore";
import { useAuth } from "./AuthContext";
import { resolveStreamNote } from "./resolveStreamNote";
import { useStreaming } from "../hooks/useStreaming";

type NoteListener = (n: Note) => void;
/** チャンネル購読が（再）確立されたことのみを知らせるリスナー。ペイロードは無い。 */
type ResyncListener = () => void;

/** Misskey互換のタイムラインチャンネル指定（バックエンド`ChannelKind`と対応）。 */
export type ChannelSpec =
  | { channel: "homeTimeline" }
  | { channel: "localTimeline" }
  | { channel: "hybridTimeline" }
  | { channel: "globalTimeline" }
  | { channel: "userList"; params: { listId: string } }
  | { channel: "hashtag"; params: { tag: string } };

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
  /**
   * タイムラインチャンネルを購読する（HomePage が表示中タブに応じて呼ぶ）。
   * `connect`/`disconnect`メッセージの送受信を内部で扱い、該当チャンネルの新着ノートのみ
   * `onNote`に届く。戻り値の関数で`disconnect`を送り購読解除する。
   * `onResync`は同一チャンネルの購読が（再）確立されるたびに呼ばれる。WebSocketが不意に
   * 切断して再接続した場合、切断中の新着を取りこぼしている可能性があるため、呼び出し元は
   * ここで最新分の再取得・マージを行う想定（HomePage参照）。
   */
  subscribeChannel: (spec: ChannelSpec, onNote: NoteListener, onResync?: ResyncListener) => () => void;
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
  subscribeChannel: () => () => {},
  registerReaction: () => () => {},
  registerNotifArrived: () => () => {},
  registerDirectMessage: () => () => {},
  dmUnreadCount: 0,
  refreshDmUnreadCount: () => {},
});

const NOTIF_KINDS = new Set(["reaction", "follow", "followAccepted", "mention", "reply", "repost", "quote"]);

export function StreamingProvider({ children }: { children: React.ReactNode }) {
  const { user } = useAuth();
  const [unread, setUnread] = useState(0);
  const [dmUnreadCount, setDmUnreadCount] = useState(0);
  const reactionListeners = useRef<Map<string, Set<ReactionListener>>>(new Map());
  const notifListeners = useRef<Set<NotifListener>>(new Set());
  const dmListeners = useRef<Set<NoteListener>>(new Set());
  /** subscription id -> {spec, onNote, onResync}。再接続時の`connect`再送、`channel`イベントの配り先に使う。 */
  const channelSubs = useRef<Map<string, { spec: ChannelSpec; onNote: NoteListener; onResync?: ResyncListener }>>(
    new Map()
  );
  // useStreaming の戻り値 send を onOpen コールバック内で使うための ref。
  // useStreaming 呼び出し自体の引数（onOpen）から戻り値（send）を参照する循環を避けるため、
  // onOpen 内では直接 send を閉じ込めず sendRef.current 経由で読む（useStreaming.ts の
  // onEventRef と同じ「レンダー中に直接代入してrefを最新に保つ」パターン）。
  const sendRef = useRef<(msg: unknown) => void>(() => {});

  const refreshDmUnreadCount = useCallback(() => {
    if (!user) return;
    api.dm.unreadCount().then((r) => setDmUnreadCount(r.count)).catch(() => {});
  }, [user]);

  useEffect(() => {
    refreshDmUnreadCount();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [user?.id]);

  const { send } = useStreaming(
    (type, body) => {
      if (type === "note") {
        // DM（visibility="direct"）は既存の`recipients`方式のまま届く。チャンネル購読不要。
        void resolveStreamNote(body).then((n) => {
          if (n.visibility === "direct") {
            dmListeners.current.forEach((cb) => cb(n));
            refreshDmUnreadCount();
          }
        });
      } else if (type === "channel") {
        const { id, type: innerType, body: innerBody } = body as {
          id: string;
          type: string;
          body: unknown;
        };
        if (innerType !== "note") return;
        const sub = channelSubs.current.get(id);
        if (!sub) return;
        void resolveStreamNote(innerBody).then((n) => sub.onNote(n));
      } else if (type === "noteUpdated") {
        const update = body as ReactionUpdate;
        reactionListeners.current.get(update.postId)?.forEach((cb) => cb(update));
      } else if (type === "pollUpdated") {
        // NoteCard は共有ストア（stores/pollVoteStore）をサブスクライブ済みのため、ここで
        // ストアを更新するだけで表示中の全 NoteCard に伝播する（専用リスナーの配線は不要、
        // registerReaction と違い NoteCard 側の useState ではなくグローバルストアなため）。
        const update = body as { postId: string; poll: NonNullable<Note["poll"]> };
        updatePollResults(update.postId, update.poll);
      } else if (NOTIF_KINDS.has(type)) {
        setUnread((u) => u + 1);
        notifListeners.current.forEach((cb) => cb());
        if (type === "followAccepted") {
          // フォロー状態は共有ストア（stores/followStatusStore）に一本化しているため、ここで直接
          // 更新するだけで、プロフィール画面・タイムライン上のフォロースイッチなど表示中の
          // 全コンポーネントに伝播する（専用リスナーの配線は不要）。
          const actor = (body as { actor?: { username?: string; domain?: string } })?.actor;
          if (actor?.username && actor?.domain) {
            setFollowStatusStore(profileQuery(actor.username, actor.domain), "accepted");
          }
        }
      }
    },
    user?.id ?? null,
    // 再接続のたびに、現在購読中の全チャンネルを再度 connect し直す
    // （WebSocket再接続でサーバー側の購読状態は失われるため）。あわせて各購読の`onResync`を
    // 呼び、切断中に取りこぼした可能性のある更新の補完を呼び出し元に促す。
    useCallback(() => {
      channelSubs.current.forEach(({ spec, onResync }, id) => {
        sendRef.current({
          type: "connect",
          body: { channel: spec.channel, id, params: "params" in spec ? spec.params : {} },
        });
        onResync?.();
      });
    }, [])
  );
  sendRef.current = send;

  const subscribeChannel = useCallback(
    (spec: ChannelSpec, onNote: NoteListener, onResync?: ResyncListener) => {
      const id = crypto.randomUUID();
      channelSubs.current.set(id, { spec, onNote, onResync });
      send({ type: "connect", body: { channel: spec.channel, id, params: "params" in spec ? spec.params : {} } });
      return () => {
        channelSubs.current.delete(id);
        send({ type: "disconnect", body: { id } });
      };
    },
    [send]
  );

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
        subscribeChannel,
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
