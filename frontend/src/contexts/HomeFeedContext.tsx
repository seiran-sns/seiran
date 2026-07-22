import { createContext, useCallback, useContext, useRef, useState } from "react";
import type { Note } from "../api/client";

/**
 * ホーム画面のフィードタブ選択・タイムラインキャッシュ・スクロール位置を
 * 「セッション内で維持」するためのストア（RightPaneContextと同様のパターン）。
 * `<Routes>` の外側に置くことで、他ページへ遷移してブラウザバック等で戻ってきても
 * タブ選択・一覧・スクロール位置がリセットされない。
 */
export type Feed = { kind: "home" } | { kind: "local" } | { kind: "list"; id: string } | { kind: "hashtag"; name: string };

export function feedKey(feed: Feed): string {
  return feed.kind === "list" ? `list:${feed.id}` : feed.kind === "hashtag" ? `hashtag:${feed.name}` : feed.kind;
}

interface FeedCacheEntry {
  notes: Note[];
  hasMore: boolean;
  scrollY: number;
}

interface HomeFeedState {
  feed: Feed;
  setFeed: (f: Feed) => void;
  getCache: (key: string) => FeedCacheEntry | undefined;
  /** 部分更新。既存エントリとマージする（notes/hasMore更新時にscrollYを消さないため）。 */
  setCache: (key: string, patch: Partial<FeedCacheEntry>) => void;
}

const HomeFeedContext = createContext<HomeFeedState | null>(null);

export function HomeFeedProvider({ children }: { children: React.ReactNode }) {
  const [feed, setFeed] = useState<Feed>({ kind: "home" });
  const cacheRef = useRef<Map<string, FeedCacheEntry>>(new Map());

  const getCache = useCallback((key: string) => cacheRef.current.get(key), []);
  const setCache = useCallback((key: string, patch: Partial<FeedCacheEntry>) => {
    const prev = cacheRef.current.get(key) ?? { notes: [], hasMore: true, scrollY: 0 };
    cacheRef.current.set(key, { ...prev, ...patch });
  }, []);

  return (
    <HomeFeedContext.Provider value={{ feed, setFeed, getCache, setCache }}>{children}</HomeFeedContext.Provider>
  );
}

export function useHomeFeed() {
  const ctx = useContext(HomeFeedContext);
  if (!ctx) throw new Error("useHomeFeed must be used within HomeFeedProvider");
  return ctx;
}
