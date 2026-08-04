import { createContext, useContext, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import i18n from "../i18n";
import { api, User, getToken, setUnauthorizedHandler } from "../api/client";
import { resolveSession } from "./authSession";

/** サーバーに保存された言語設定（#55）があれば、ブラウザ判定・localStorage より優先して適用する。 */
function applyLanguagePreference(user: User) {
  if (user.language_preference) {
    i18n.changeLanguage(user.language_preference);
  }
}

interface AuthContextValue {
  user: User | null;
  loading: boolean;
  login: (token: string, user: User) => void;
  logout: () => void;
}

const AuthContext = createContext<AuthContextValue>({
  user: null,
  loading: true,
  login: () => {},
  logout: () => {},
});

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [loading, setLoading] = useState(true);
  const navigate = useNavigate();

  useEffect(() => {
    if (!getToken()) {
      setLoading(false);
      return;
    }

    let cancelled = false;

    resolveSession(() => api.auth.me()).then((result) => {
      if (cancelled) return;
      if (result.kind === "authenticated") {
        setUser(result.user);
        applyLanguagePreference(result.user);
      } else if (result.kind === "expired") {
        // 明示的な認証失効（401）だけがログアウトすべき理由。
        // それ以外（バックエンド再起動中の接続失敗・5xx等）でトークンを消すと、
        // 再起動のたびにログイン状態が失われてしまう（#108）。
        localStorage.removeItem("seiran_token");
      }
      // "unresolved" はトークンを保持したまま諦める。次回のマウント（再読み込み等）で
      // バックエンドが復旧していればログイン状態が回復する。
      setLoading(false);
    });

    return () => {
      cancelled = true;
    };
  }, []);

  function login(token: string, user: User) {
    localStorage.setItem("seiran_token", token);
    setUser(user);
    applyLanguagePreference(user);
  }

  function logout() {
    localStorage.removeItem("seiran_token");
    setUser(null);
  }

  // トークン失効時（401）にログイン画面へ誘導する共通処理。
  // 任意のAPIの401だけでは即座にログアウトせず、/auth/me でセッション失効を
  // 再確認する。バックエンド停止中にプロキシ等から一時的な401が返っても、
  // トークンと表示中のログイン状態を失わないため（#108）。
  useEffect(() => {
    let active = true;
    let verificationInFlight = false;

    setUnauthorizedHandler(() => {
      if (verificationInFlight) return;
      verificationInFlight = true;

      void resolveSession(() => api.auth.me())
        .then((result) => {
          if (!active) return;
          if (result.kind === "expired") {
            logout();
            navigate("/login", { replace: true });
          } else if (result.kind === "authenticated") {
            setUser(result.user);
            applyLanguagePreference(result.user);
          }
        })
        .finally(() => {
          verificationInFlight = false;
        });
    });
    return () => {
      active = false;
      setUnauthorizedHandler(null);
    };
  }, [navigate]);

  return (
    <AuthContext.Provider value={{ user, loading, login, logout }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  return useContext(AuthContext);
}
