import { createContext, useContext, useState } from "react";
import type { NotificationItem } from "../api/client";

interface NotificationsPanelCache {
  items: NotificationItem[];
  hasMore: boolean;
}

/**
 * 右ペインのサブタブ選択状態を「セッション内で維持」するためのストア（Doc5 §2.4）。
 *
 * 中央ペインでポスト A → ポスト B へ遷移しても、右ペインのアクティブなサブタブ
 * インデックスはリセットされず保持される。これにより TL を上から順にクリックする
 * だけで、常に同じモード（例: 「前後のポスト」）で文脈を覗き見できる。
 */
interface RightPaneState {
  /** ホーム画面の右ペインタブ（0: クイック通知, 1: トレンド＆検索）。トレンド集計はまだ未実装のため、機能しているクイック通知をデフォルトタブにしている。 */
  timelineTab: number;
  setTimelineTab: (i: number) => void;
  /** ポスト詳細の右ペインタブ（0: 投稿者, 1: 返信, 2: 前後のポスト, 3: リアクション, 4: リポスト）。 */
  noteDetailTab: number;
  setNoteDetailTab: (i: number) => void;
  /** 「前後のポスト」タブのスクロール位置（ノートID→scrollTop）。ブラウザバックで同じポストの
   * 詳細画面に戻った際に再現するため、セッション内（インメモリ）で保持する（#226）。 */
  noteContextScroll: Record<string, number>;
  setNoteContextScroll: (noteId: string, scrollTop: number) => void;
  /** ポスト詳細画面でスレッドを遡って読み込んだ返信先ポストのID列（対象ポストID→古い順、
   * 直近の親が末尾）。ブラウザバックで同じポストへ戻った際に遡り状態を再現するため、
   * セッション内（インメモリ）で保持する。 */
  noteAncestorIds: Record<string, string[]>;
  setNoteAncestorIds: (noteId: string, ids: string[]) => void;
  /** ポスト詳細画面（中央ペイン）のスクロール位置（ノートID→window.scrollY）。上記と同じ理由で
   * ブラウザバック時に再現するためセッション内（インメモリ）で保持する。 */
  noteDetailScrollY: Record<string, number>;
  setNoteDetailScrollY: (noteId: string, y: number) => void;
  /** 右ペイン「クイック通知」タブ（Home/Search画面）のスクロール位置（`.rightScroll`の
   * scrollTop）。他画面へ遷移して戻ってきた際・タブを行き来した際に再現するため、
   * セッション内（インメモリ）で保持する。 */
  notifPanelScrollY: number;
  setNotifPanelScrollY: (y: number) => void;
  /** 上記と同じ【クイック通知】タブの一覧本体（追加読み込み分含む）。スクロール位置だけでなく
   * これも記憶しないと、復帰時の再フェッチで先頭ページ分だけになり一覧の実高さが足りず
   * スクロール位置の復元が壊れる（無限スクロールで深く読み込んでいた場合）。 */
  notifPanelCache: NotificationsPanelCache | undefined;
  setNotifPanelCache: (cache: NotificationsPanelCache) => void;
  /** 通知一覧画面（中央ペインでwindowスクロールする独立ページ）のスクロール位置。 */
  notificationsPageScrollY: number;
  setNotificationsPageScrollY: (y: number) => void;
  /** 通知一覧画面の一覧本体。上記`notifPanelCache`と同じ理由で必要。 */
  notificationsPageCache: NotificationsPanelCache | undefined;
  setNotificationsPageCache: (cache: NotificationsPanelCache) => void;
}

const RightPaneContext = createContext<RightPaneState>({
  timelineTab: 0,
  setTimelineTab: () => {},
  noteDetailTab: 0,
  setNoteDetailTab: () => {},
  noteContextScroll: {},
  setNoteContextScroll: () => {},
  noteAncestorIds: {},
  setNoteAncestorIds: () => {},
  noteDetailScrollY: {},
  setNoteDetailScrollY: () => {},
  notifPanelScrollY: 0,
  setNotifPanelScrollY: () => {},
  notifPanelCache: undefined,
  setNotifPanelCache: () => {},
  notificationsPageScrollY: 0,
  setNotificationsPageScrollY: () => {},
  notificationsPageCache: undefined,
  setNotificationsPageCache: () => {},
});

export function RightPaneProvider({ children }: { children: React.ReactNode }) {
  const [timelineTab, setTimelineTab] = useState(0);
  const [noteDetailTab, setNoteDetailTab] = useState(0);
  const [noteContextScroll, setNoteContextScrollState] = useState<Record<string, number>>({});
  const setNoteContextScroll = (noteId: string, scrollTop: number) => {
    setNoteContextScrollState((prev) => ({ ...prev, [noteId]: scrollTop }));
  };
  const [noteAncestorIds, setNoteAncestorIdsState] = useState<Record<string, string[]>>({});
  const setNoteAncestorIds = (noteId: string, ids: string[]) => {
    setNoteAncestorIdsState((prev) => ({ ...prev, [noteId]: ids }));
  };
  const [noteDetailScrollY, setNoteDetailScrollYState] = useState<Record<string, number>>({});
  const setNoteDetailScrollY = (noteId: string, y: number) => {
    setNoteDetailScrollYState((prev) => ({ ...prev, [noteId]: y }));
  };
  const [notifPanelScrollY, setNotifPanelScrollY] = useState(0);
  const [notifPanelCache, setNotifPanelCache] = useState<NotificationsPanelCache | undefined>(undefined);
  const [notificationsPageScrollY, setNotificationsPageScrollY] = useState(0);
  const [notificationsPageCache, setNotificationsPageCache] = useState<NotificationsPanelCache | undefined>(
    undefined
  );
  return (
    <RightPaneContext.Provider
      value={{
        timelineTab,
        setTimelineTab,
        noteDetailTab,
        setNoteDetailTab,
        noteContextScroll,
        setNoteContextScroll,
        noteAncestorIds,
        setNoteAncestorIds,
        noteDetailScrollY,
        setNoteDetailScrollY,
        notifPanelScrollY,
        setNotifPanelScrollY,
        notifPanelCache,
        setNotifPanelCache,
        notificationsPageScrollY,
        setNotificationsPageScrollY,
        notificationsPageCache,
        setNotificationsPageCache,
      }}
    >
      {children}
    </RightPaneContext.Provider>
  );
}

export function useRightPane() {
  return useContext(RightPaneContext);
}
