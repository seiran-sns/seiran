import { useCallback, useRef, useState } from "react";

/**
 * 「末尾要素の id を until_id カーソルとして次ページを取得し、重複除去して末尾へ追記する」
 * というカーソルページネーションの共通ロジック。
 *
 * HomePage / ProfilePage / HashtagPage / ListDetailPage / NotificationsPanel の5箇所に
 * ほぼ同一の実装（`itemsRef` 同期・`loadingMoreRef` によるガード・`until_id` 算出・重複除去
 * `Set`）が分散していたものを統合する。取得失敗時は必ず `onError` を呼ぶため、
 * 各ページで `.catch()` が抜けて「エラー時に無限スクロールが無言で止まる」問題も解消する。
 *
 * 初回ロードは呼び出し元の既存 `useEffect`（Promise.all との組み合わせ・notFound 判定等が
 * ページごとに異なるため）に任せ、この hook が返す `setItems`/`setHasMore` で結果を渡す。
 */
export function useCursorPagination<T>(
  fetchPage: (untilId: string) => Promise<T[]>,
  getId: (item: T) => string,
  pageSize: number,
  onError: (err: unknown) => void
) {
  const [items, setItems] = useState<T[]>([]);
  const [hasMore, setHasMore] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const itemsRef = useRef<T[]>([]);
  const loadingMoreRef = useRef(false);
  itemsRef.current = items;

  // fetchPage/getId/onError は呼び出し側で feed 切替等のたびに新しい関数参照になりうる。
  // ref 経由で常に最新を参照することで、loadMore 自身の参照は安定させたまま
  // （sentinel の IntersectionObserver 再アタッチを増やさないまま）古いクロージャを
  // 掴み続ける（＝切替後も切替前のフィードを取得し続ける）バグを避ける。
  const fetchPageRef = useRef(fetchPage);
  fetchPageRef.current = fetchPage;
  const getIdRef = useRef(getId);
  getIdRef.current = getId;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  const loadMore = useCallback(() => {
    if (loadingMoreRef.current || itemsRef.current.length === 0) return;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    const getIdFn = getIdRef.current;
    const untilId = getIdFn(itemsRef.current[itemsRef.current.length - 1]);
    fetchPageRef
      .current(untilId)
      .then((rows) => {
        setItems((prev) => {
          const seen = new Set(prev.map(getIdFn));
          const fresh = rows.filter((r) => !seen.has(getIdFn(r)));
          return [...prev, ...fresh];
        });
        setHasMore(rows.length >= pageSize);
      })
      .catch((err) => onErrorRef.current(err))
      .finally(() => {
        loadingMoreRef.current = false;
        setLoadingMore(false);
      });
  }, [pageSize]);

  return { items, setItems, hasMore, setHasMore, loadingMore, loadMore };
}
