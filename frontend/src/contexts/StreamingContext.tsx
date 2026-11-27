import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { api, Note, ReactionSummary } from "../api/client";
import { profileQuery } from "../lib/format";
import { getAdminTopics } from "../lib/roles";
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
  | { channel: "localTimelineModeration" }
  | { channel: "hybridTimeline" }
  | { channel: "globalTimeline" }
  | { channel: "userList"; params: { listId: string } }
  | { channel: "hashtag"; params: { tag: string } };

/** `noteUpdated`（リアクション追加/切替/取消）のライブ更新1件。 */
export interface ReactionUpdate {
  postId: string;
  /** 更新後の権威的な集計（`reactedByMe` は含まない。閲覧者ごとに異なるため）。 */
  reactions: Pick<ReactionSummary, "emoji" | "count" | "emojiUrl">[];
  /** 操作した本人の actor_id（Snowflake ID、文字列）。自分自身の別タブ/端末からの操作かの判定に使う。ブリッジ関係にある別の投稿への操作で件数だけが変わったときは null。 */
  reactorActorId: string | null;
  /** 操作後、reactor がこの投稿に付けているリアクション（切替/追加なら絵文字、取消なら null）。 */
  reactorEmoji: string | null;
}

type ReactionListener = (u: ReactionUpdate) => void;

/** bio/profile_fields中のURL解決完了（`Job::LinkResolve`）1件（#リンク解決）。 */
export interface LinkResolvedInfo {
  url: string;
  kind: "actor" | "post";
  username?: string;
  domain?: string;
  actorType?: string;
  actorId?: string;
  avatarUrl?: string;
  postId?: string;
}

type LinkResolvedListener = (info: LinkResolvedInfo) => void;

/**
 * 通知系イベント（reaction/follow/followAccepted）が届いたことのみを知らせるリスナー。
 * ペイロードは使わない。通知の永続化（`POST /api/i/notifications`）に一本化したため、
 * WS ペイロードは「新着があったので再取得せよ」という即時性のためのシグナルに過ぎない。
 */
type NotifListener = () => void;

interface StreamingValue {
  /** 未読通知の件数（サーバーの未読数）。通知一覧を表示している間は常に0。 */
  unread: number;
  markRead: () => void;
  /** 既読にしながら通知一覧を表示している（NotificationsPanel が絞り込み無しで出ている）間、
   * 登録しておく。その間に届いた通知はすぐ既読になるので、未読数を取りに行かず0のままにする
   * （取りに行くと、既読にする前の一瞬だけバッジが出てしまう）。戻り値で解除。 */
  registerNotifViewer: () => () => void;
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
  /** bio/profile_fields内リンクの非同期解決完了を購読する（ProfilePageが使用）。戻り値で解除。
   * ログイン中クライアント全員へ無条件配信されるため、呼び出し側で対象URLかを判定すること。 */
  registerLinkResolved: (cb: LinkResolvedListener) => () => void;
  /** 予約の投稿・失敗・他のタブでの操作（`scheduledPostsChanged`）を購読する（予約一覧が
   * 使用）。戻り値で解除。 */
  registerScheduledPostsChanged: (cb: () => void) => () => void;
  /** DM新着（visibility="direct"のnoteイベント）を購読する（MessagesPageが使用）。戻り値で解除。 */
  registerDirectMessage: (cb: NoteListener) => () => void;
  /** 未読のあるDMセッション数（左ペインバッジ用）。 */
  dmUnreadCount: number;
  /** サーバーから未読数を再取得する（既読操作後・ページロード時に呼ぶ）。 */
  refreshDmUnreadCount: () => void;
  /** 承認待ちフォローリクエスト件数（承認制フォロー中のみ非0、設定アイコン・
   * 「承認待ちフォロー」メニュー項目のバッジ用）。 */
  followRequestCount: number;
  /** サーバーから承認待ち件数を再取得する（承認/拒否操作後・ページロード時に呼ぶ）。 */
  refreshFollowRequestCount: () => void;
  /** 予約投稿の件数。投稿待ち（`pending`）は「機能」メニューの🕒印、投稿できなかったもの
   * （`failed`）は件数バッジにする。 */
  scheduledPostCounts: { pending: number; failed: number };
  /** サーバーから予約投稿の件数を再取得する（予約の操作後に呼ぶ。実行ループの投稿・失敗は
   * ストリーミングの`scheduledPostsChanged`で取り直す）。 */
  refreshScheduledPostCounts: () => void;
  /** 未処理の通報の件数（通報を扱えるロールだけ。管理メニュー・通報タブのバッジ用）。 */
  openReportCount: number;
  /** サーバーから未処理の通報の件数を再取得する（通報の処理後に呼ぶ）。 */
  refreshOpenReportCount: () => void;
}

const StreamingContext = createContext<StreamingValue>({
  unread: 0,
  markRead: () => {},
  registerNotifViewer: () => () => {},
  subscribeChannel: () => () => {},
  registerReaction: () => () => {},
  registerNotifArrived: () => () => {},
  registerLinkResolved: () => () => {},
  registerDirectMessage: () => () => {},
  registerScheduledPostsChanged: () => () => {},
  dmUnreadCount: 0,
  refreshDmUnreadCount: () => {},
  followRequestCount: 0,
  refreshFollowRequestCount: () => {},
  scheduledPostCounts: { pending: 0, failed: 0 },
  refreshScheduledPostCounts: () => {},
  openReportCount: 0,
  refreshOpenReportCount: () => {},
});

/** 未処理の通報の件数を取り直す間隔。新しい通報はストリーミングで届かないので定期的に確かめる。 */
const OPEN_REPORT_POLL_MS = 60_000;

const NOTIF_KINDS = new Set([
  "reaction",
  "follow",
  "followAccepted",
  "followRequest",
  "mention",
  "reply",
  "repost",
  "quote",
  "moveRefollowed",
  "moveAlreadyFollowing",
]);

export function StreamingProvider({ children }: { children: React.ReactNode }) {
  const { user } = useAuth();
  const [unread, setUnread] = useState(0);
  const [dmUnreadCount, setDmUnreadCount] = useState(0);
  const [followRequestCount, setFollowRequestCount] = useState(0);
  const [scheduledPostCounts, setScheduledPostCounts] = useState({ pending: 0, failed: 0 });
  const [openReportCount, setOpenReportCount] = useState(0);
  const canHandleReports = getAdminTopics(user?.role).includes("reports");
  const reactionListeners = useRef<Map<string, Set<ReactionListener>>>(new Map());
  const notifListeners = useRef<Set<NotifListener>>(new Set());
  const linkResolvedListeners = useRef<Set<LinkResolvedListener>>(new Set());
  const dmListeners = useRef<Set<NoteListener>>(new Set());
  const scheduledListeners = useRef<Set<() => void>>(new Set());
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

  // 未読数の取得の世代。既読にしたら進め、それより前に始めた取得の結果は捨てる
  // （既読にした直後に、既読にする前の件数で上書きしないため）。
  const unreadGenRef = useRef(0);
  const notifViewersRef = useRef(0);
  const refreshUnread = useCallback(() => {
    if (!user || notifViewersRef.current > 0) return;
    const gen = unreadGenRef.current;
    api.notifications
      .unreadCount()
      .then((r) => {
        if (unreadGenRef.current === gen && notifViewersRef.current === 0) setUnread(r.count);
      })
      .catch(() => {});
  }, [user]);

  const refreshFollowRequestCount = useCallback(() => {
    if (!user) return;
    api.followRequests.count().then((r) => setFollowRequestCount(r.count)).catch(() => {});
  }, [user]);

  const refreshScheduledPostCounts = useCallback(() => {
    if (!user) return;
    api.scheduledPosts.counts().then(setScheduledPostCounts).catch(() => {});
  }, [user]);

  const refreshOpenReportCount = useCallback(() => {
    if (!canHandleReports) return;
    api.admin.openReportCount().then((r) => setOpenReportCount(r.count)).catch(() => {});
  }, [canHandleReports]);

  useEffect(() => {
    if (!canHandleReports) {
      setOpenReportCount(0);
      return;
    }
    refreshOpenReportCount();
    const timer = setInterval(refreshOpenReportCount, OPEN_REPORT_POLL_MS);
    return () => clearInterval(timer);
  }, [canHandleReports, refreshOpenReportCount, user?.id]);

  useEffect(() => {
    refreshDmUnreadCount();
    refreshFollowRequestCount();
    refreshScheduledPostCounts();
    refreshUnread();
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
        if (!channelSubs.current.has(id)) return;
        // resolveStreamNote は実際にAPIへ問い合わせる非同期処理のため、この待機中に
        // タブ切替等で該当チャンネルが disconnect される可能性がある。ここで sub を
        // 先に確定させてしまうと、await後に古い（すでに購読解除された）タブ用の
        // onNote をそのまま呼んでしまい、切替先のタイムラインへ誤って挿入されてしまう
        // （例: Home→Local切替直後、Local先頭にHomeの投稿が挿入される）。
        // 必ず await の後に改めて channelSubs.current を引き直し、購読が生きている
        // 場合だけ配る。
        void resolveStreamNote(innerBody).then((n) => {
          channelSubs.current.get(id)?.onNote(n);
        });
      } else if (type === "noteUpdated") {
        const update = body as ReactionUpdate;
        reactionListeners.current.get(update.postId)?.forEach((cb) => cb(update));
      } else if (type === "pollUpdated") {
        // NoteCard は共有ストア（stores/pollVoteStore）をサブスクライブ済みのため、ここで
        // ストアを更新するだけで表示中の全 NoteCard に伝播する（専用リスナーの配線は不要、
        // registerReaction と違い NoteCard 側の useState ではなくグローバルストアなため）。
        const update = body as { postId: string; poll: NonNullable<Note["poll"]> };
        updatePollResults(update.postId, update.poll);
      } else if (type === "scheduledPostsChanged") {
        refreshScheduledPostCounts();
        scheduledListeners.current.forEach((cb) => cb());
      } else if (type === "linkResolved") {
        const info = body as LinkResolvedInfo;
        linkResolvedListeners.current.forEach((cb) => cb(info));
      } else if (NOTIF_KINDS.has(type)) {
        refreshUnread();
        notifListeners.current.forEach((cb) => cb());
        if (type === "followRequest") {
          refreshFollowRequestCount();
        }
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
    // 呼び、切断中に取りこぼした可能性のある更新の補完を呼び出し元に促す。表示中ノートの
    // `subNote`も同じ理由で送り直す（接続前に登録されたぶんもここで届く）。
    useCallback(() => {
      reactionListeners.current.forEach((_, noteId) => {
        sendRef.current({ type: "subNote", body: { id: noteId } });
      });
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

  // 表示中のノートをサーバーへ`subNote`で購読する。投稿者をフォローしていないノート
  // （ローカル・グローバル・リスト・リポスト等）のリアクション（`noteUpdated`）とアンケートの
  // 票数（`pollUpdated`）の更新は購読しないと届かない。NoteCard はどれもリアクションの
  // リスナーを登録するので、アンケートの購読もこれで兼ねる。
  // 同じノートを複数のカードで出していても、購読はノートごとに1つにまとめる。
  const registerReaction = useCallback((noteId: string, cb: ReactionListener) => {
    let set = reactionListeners.current.get(noteId);
    if (!set) {
      set = new Set();
      reactionListeners.current.set(noteId, set);
      sendRef.current({ type: "subNote", body: { id: noteId } });
    }
    set.add(cb);
    return () => {
      const s = reactionListeners.current.get(noteId);
      if (!s) return;
      s.delete(cb);
      if (s.size === 0) {
        reactionListeners.current.delete(noteId);
        sendRef.current({ type: "unsubNote", body: { id: noteId } });
      }
    };
  }, []);

  const registerNotifArrived = useCallback((cb: NotifListener) => {
    notifListeners.current.add(cb);
    return () => {
      notifListeners.current.delete(cb);
    };
  }, []);

  const registerLinkResolved = useCallback((cb: LinkResolvedListener) => {
    linkResolvedListeners.current.add(cb);
    return () => {
      linkResolvedListeners.current.delete(cb);
    };
  }, []);

  const registerScheduledPostsChanged = useCallback((cb: () => void) => {
    scheduledListeners.current.add(cb);
    return () => {
      scheduledListeners.current.delete(cb);
    };
  }, []);

  const registerDirectMessage = useCallback((cb: NoteListener) => {
    dmListeners.current.add(cb);
    return () => {
      dmListeners.current.delete(cb);
    };
  }, []);

  const markRead = useCallback(() => {
    unreadGenRef.current += 1;
    setUnread(0);
  }, []);

  const registerNotifViewer = useCallback(() => {
    notifViewersRef.current += 1;
    unreadGenRef.current += 1;
    setUnread(0);
    return () => {
      notifViewersRef.current -= 1;
      // 一覧を閉じたあとに届いた分から数え直す（表示中に届いた分は既読になっている）。
      refreshUnread();
    };
  }, [refreshUnread]);

  return (
    <StreamingContext.Provider
      value={{
        unread,
        markRead,
        registerNotifViewer,
        subscribeChannel,
        registerReaction,
        registerNotifArrived,
        registerLinkResolved,
        registerDirectMessage,
        dmUnreadCount,
        refreshDmUnreadCount,
        followRequestCount,
        refreshFollowRequestCount,
        scheduledPostCounts,
        refreshScheduledPostCounts,
        registerScheduledPostsChanged,
        openReportCount,
        refreshOpenReportCount,
      }}
    >
      {children}
    </StreamingContext.Provider>
  );
}

export function useStreamingContext() {
  return useContext(StreamingContext);
}
