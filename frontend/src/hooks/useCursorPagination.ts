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
 *
 * `initial`（省略可）を渡すと`items`/`hasMore`をその値で初期化する。呼び出し元が
 * セッション内キャッシュを持っている場合、初回レンダーの時点から復元済みの内容を
 * 表示するために使う。`useEffect`経由で`setItems`するのでは、そのeffectが走るまでの
 * 最初の1回のレンダーが空一覧になってしまい、その一瞬だけ実高さが縮んでスクロール位置の
 * 復元が壊れる（`window.scrollY`がブラウザに強制的にクランプされる）。
 *
 * `resetKey`（省略可）は「今の一覧がどの取得対象のものか」を表す。値が変わった時点で
 * 取得中の次ページは破棄し、完了しても一覧へ追記しない。破棄しないと、切替前の対象の
 * 次ページが切替後の一覧の末尾に混ざる（ホームのタブ切替でLTLにGTLの投稿が並ぶ等）。
 */
export function useCursorPagination<T>(
  fetchPage: (untilId: string) => Promise<T[]>,
  getId: (item: T) => string,
  pageSize: number,
  onError: (err: unknown) => void,
  initial?: { items: T[]; hasMore: boolean },
  resetKey?: string
) {
  const [items, setItems] = useState<T[]>(initial?.items ?? []);
  const [hasMore, setHasMore] = useState(initial?.hasMore ?? true);
  const itemsRef = useRef<T[]>([]);
  itemsRef.current = items;

  // resetKeyが変わるたびに進む世代番号。A→B→Aと戻った場合も、最初のAで始めた取得は
  // 別世代として破棄する（その間に一覧はキャッシュ復元等で差し替わっているため）。
  const resetKeyRef = useRef(resetKey);
  const generationRef = useRef(0);
  if (resetKeyRef.current !== resetKey) {
    resetKeyRef.current = resetKey;
    generationRef.current += 1;
  }
  // 取得中の次ページの世代（無ければnull）。古い世代の取得が残っていても、現世代の
  // loadMoreは止めず、読み込み中表示も出さない。
  const [loadingGeneration, setLoadingGeneration] = useState<number | null>(null);
  const loadingGenerationRef = useRef<number | null>(null);
  const loadingMore = loadingGeneration === generationRef.current;

  // fetchPage/getId/onError は呼び出し側で feed 切替等のたびに新しい関数参照になりうる。
  // ref 経由で常に最新を参照することで、loadMore 自身の参照は安定させたまま
  // （sentinel の IntersectionObserver 再アタッチを増やさないまま）古いクロージャを
  // 掴み続ける（＝切替後も切替前のフィードを取得し続ける）のを避ける。
  const fetchPageRef = useRef(fetchPage);
  fetchPageRef.current = fetchPage;
  const getIdRef = useRef(getId);
  getIdRef.current = getId;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  const loadMore = useCallback(() => {
    const generation = generationRef.current;
    if (loadingGenerationRef.current === generation || itemsRef.current.length === 0) return;
    loadingGenerationRef.current = generation;
    setLoadingGeneration(generation);
    const getIdFn = getIdRef.current;
    const untilId = getIdFn(itemsRef.current[itemsRef.current.length - 1]);
    fetchPageRef
      .current(untilId)
      .then((rows) => {
        if (generationRef.current !== generation) return;
        setItems((prev) => {
          const seen = new Set(prev.map(getIdFn));
          const fresh = rows.filter((r) => !seen.has(getIdFn(r)));
          return [...prev, ...fresh];
        });
        setHasMore(rows.length >= pageSize);
      })
      .catch((err) => {
        if (generationRef.current === generation) onErrorRef.current(err);
      })
      .finally(() => {
        if (loadingGenerationRef.current !== generation) return;
        loadingGenerationRef.current = null;
        setLoadingGeneration(null);
      });
  }, [pageSize]);

  return { items, setItems, hasMore, setHasMore, loadingMore, loadMore };
}
