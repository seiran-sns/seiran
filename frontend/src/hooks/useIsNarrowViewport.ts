import { useEffect, useState } from "react";

/** AppShell.module.css の右ペイン非表示ブレークポイントと合わせた幅判定。 */
const NARROW_BREAKPOINT_PX = 1400;

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
