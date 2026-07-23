import { createContext, ReactNode, useContext, useEffect, useRef } from "react";
import { useLocation, useNavigate, useNavigationType } from "react-router-dom";

/**
 * SPA内で辿ってきたナビゲーション（PUSH）の深さを追跡し、「戻る」ボタンの挙動を
 * 統一するためのストア。直接URLを踏んだ・リロードした等でSPA内の戻り先が無い場合、
 * `navigate(-1)` はアプリ外（あるいは存在しない履歴）に出てしまうため、その場合は
 * ホーム画面を履歴に積んでそこへ遷移する。
 */
interface NavigationHistoryState {
  goBack: () => void;
}

const NavigationHistoryContext = createContext<NavigationHistoryState>({
  goBack: () => {},
});

export function NavigationHistoryProvider({ children }: { children: ReactNode }) {
  const navigate = useNavigate();
  const navigationType = useNavigationType();
  const location = useLocation();
  const depthRef = useRef(0);
  const prevKeyRef = useRef(location.key);

  useEffect(() => {
    if (location.key === prevKeyRef.current) return;
    prevKeyRef.current = location.key;
    if (navigationType === "PUSH") {
      depthRef.current += 1;
    } else if (navigationType === "POP") {
      depthRef.current = Math.max(0, depthRef.current - 1);
    }
  }, [location.key, navigationType]);

  function goBack() {
    if (depthRef.current <= 0) {
      navigate("/");
      return;
    }
    navigate(-1);
  }

  return (
    <NavigationHistoryContext.Provider value={{ goBack }}>{children}</NavigationHistoryContext.Provider>
  );
}

export function useGoBack() {
  return useContext(NavigationHistoryContext).goBack;
}
