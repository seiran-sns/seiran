import { useCallback, useRef } from "react";

/**
 * 無限スクロールの sentinel（末尾検知用の空要素）に付ける callback ref を返す。
 *
 * `useEffect` + `ref` オブジェクトの組み合わせだと、sentinel の DOM ノードが
 * 新しい要素に置き換わった際（tab切り替え直後に `hasMore`/一覧件数がたまたま
 * 前後で同値になる等）に observer の再アタッチが漏れるバグがあった
 * （`NoteList.tsx` で発見・修正済みのパターン）。callback ref は要素の生成・破棄の
 * たびに React が必ず呼ぶため、この種の再アタッチ漏れが起きない。
 */
export function useInfiniteScrollSentinel<E extends Element>(onLoadMore: (() => void) | undefined, hasMore: boolean | undefined) {
  const observerRef = useRef<IntersectionObserver | null>(null);

  return useCallback(
    (el: E | null) => {
      observerRef.current?.disconnect();
      observerRef.current = null;
      if (!el || !onLoadMore || !hasMore) return;
      const observer = new IntersectionObserver(
        (entries) => {
          if (entries[0]?.isIntersecting) onLoadMore();
        },
        { rootMargin: "200px" }
      );
      observer.observe(el);
      observerRef.current = observer;
    },
    [onLoadMore, hasMore]
  );
}
