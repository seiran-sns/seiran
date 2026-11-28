import { useEffect, useState } from "react";

/** AppShell.module.css の右ペイン非表示ブレークポイント（`.right`が2ペイン側へ
 * 折り返される`max-width: 1220px`）と合わせた幅判定。値がずれると、CSS側はまだ
 * 3ペインを維持しているのにここだけ先に右ペインの中身を空にしてしまい、
 * 中身のない`.right`の空箱だけが表示される。 */
const NARROW_BREAKPOINT_PX = 1220;

/** AppShell.module.css のモバイルレイアウト（左ペインを隠してフローティングボタンにする
 * `max-width: 768px`）と合わせた幅判定。 */
const MOBILE_BREAKPOINT_PX = 768;

function useMaxWidth(px: number): boolean {
  const [matches, setMatches] = useState(false);

  useEffect(() => {
    const mql = window.matchMedia(`(max-width: ${px}px)`);
    setMatches(mql.matches);
    const handler = (e: MediaQueryListEvent) => setMatches(e.matches);
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  }, [px]);

  return matches;
}

/**
 * 右ペインが非表示になる狭幅ビューポートかどうかを返す（`ProfilePage`/`ListsSettingsPage`
 * に同一実装が複製されていたものを統合）。
 */
export function useIsNarrowViewport(): boolean {
  return useMaxWidth(NARROW_BREAKPOINT_PX);
}

/** モバイルレイアウト（中央と右のペインを縦に積む1カラム）のビューポートかどうかを返す。 */
export function useIsMobileViewport(): boolean {
  return useMaxWidth(MOBILE_BREAKPOINT_PX);
}
