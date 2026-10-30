import { useEffect, useRef, useState } from "react";
import { useAuth } from "../contexts/AuthContext";

/**
 * ログイン済みから未ログインへ変わるたびに増える数。表示中のデータの取得 effect の依存に
 * 入れると、ログアウトと同時に未ログイン用の可視性で取り直せる（リモートのポスト・
 * ユーザーはログインしていないと見えないため、表示が残らないようにする）。
 * 起動時のセッション解決（未ログイン→ログイン）では増えない。
 */
export function useLogoutCount(): number {
  const { user } = useAuth();
  const loggedIn = !!user;
  const prev = useRef(loggedIn);
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (prev.current && !loggedIn) setCount((c) => c + 1);
    prev.current = loggedIn;
  }, [loggedIn]);
  return count;
}
