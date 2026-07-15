import { createContext, useContext, useState } from "react";

/**
 * 右ペインのサブタブ選択状態を「セッション内で維持」するためのストア（Doc5 §2.4）。
 *
 * 中央ペインでポスト A → ポスト B へ遷移しても、右ペインのアクティブなサブタブ
 * インデックスはリセットされず保持される。これにより TL を上から順にクリックする
 * だけで、常に同じモード（例: 「投稿主の前後の投稿」）で文脈を覗き見できる。
 */
interface RightPaneState {
  /** ホーム画面の右ペインタブ（0: クイック通知, 1: トレンド＆検索）。トレンド集計はまだ未実装のため、機能しているクイック通知をデフォルトタブにしている。 */
  timelineTab: number;
  setTimelineTab: (i: number) => void;
  /** ポスト詳細の右ペインタブ（0: 投稿主の前後, 1: リアクション）。 */
  noteDetailTab: number;
  setNoteDetailTab: (i: number) => void;
}

const RightPaneContext = createContext<RightPaneState>({
  timelineTab: 0,
  setTimelineTab: () => {},
  noteDetailTab: 0,
  setNoteDetailTab: () => {},
});

export function RightPaneProvider({ children }: { children: React.ReactNode }) {
  const [timelineTab, setTimelineTab] = useState(0);
  const [noteDetailTab, setNoteDetailTab] = useState(0);
  return (
    <RightPaneContext.Provider
      value={{ timelineTab, setTimelineTab, noteDetailTab, setNoteDetailTab }}
    >
      {children}
    </RightPaneContext.Provider>
  );
}

export function useRightPane() {
  return useContext(RightPaneContext);
}
