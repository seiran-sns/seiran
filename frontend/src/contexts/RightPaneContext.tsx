import { createContext, useContext, useState } from "react";

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
}

const RightPaneContext = createContext<RightPaneState>({
  timelineTab: 0,
  setTimelineTab: () => {},
  noteDetailTab: 0,
  setNoteDetailTab: () => {},
  noteContextScroll: {},
  setNoteContextScroll: () => {},
});

export function RightPaneProvider({ children }: { children: React.ReactNode }) {
  const [timelineTab, setTimelineTab] = useState(0);
  const [noteDetailTab, setNoteDetailTab] = useState(0);
  const [noteContextScroll, setNoteContextScrollState] = useState<Record<string, number>>({});
  const setNoteContextScroll = (noteId: string, scrollTop: number) => {
    setNoteContextScrollState((prev) => ({ ...prev, [noteId]: scrollTop }));
  };
  return (
    <RightPaneContext.Provider
      value={{
        timelineTab,
        setTimelineTab,
        noteDetailTab,
        setNoteDetailTab,
        noteContextScroll,
        setNoteContextScroll,
      }}
    >
      {children}
    </RightPaneContext.Provider>
  );
}

export function useRightPane() {
  return useContext(RightPaneContext);
}
