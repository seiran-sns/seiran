import { useEffect, useState } from "react";

/** AppShell.module.css の右ペイン非表示ブレークポイント（`.right`が2ペイン側へ
 * 折り返される`max-width: 1220px`）と合わせた幅判定。値がずれると、CSS側はまだ
 * 3ペインを維持しているのにここだけ先に右ペインの中身を空にしてしまい、
 * 中身のない`.right`の空箱だけが表示される（実機確認済みの回帰）。 */
const NARROW_BREAKPOINT_PX = 1220;

/**
 * 右ペインが非表示になる狭幅ビューポートかどうかを返す（`ProfilePage`/`ListsSettingsPage`
 * に同一実装が複製されていたものを統合）。
 */
export function useIsNarrowViewport(): boolean {
  const [isNarrow, setIsNarrow] = useState(false);

  useEffect(() => {
    const mql = window.matchMedia(`(max-width: ${NARROW_BREAKPOINT_PX}px)`);
    setIsNarrow(mql.matches);
    const handler = (e: MediaQueryListEvent) => setIsNarrow(e.matches);
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  }, []);

  return isNarrow;
}
