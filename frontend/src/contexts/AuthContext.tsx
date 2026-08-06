import { createContext, useContext, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import i18n from "../i18n";
import { api, User, getToken, setUnauthorizedHandler } from "../api/client";
import { resolveSession } from "./authSession";

/**
 * JWTのスライディング延命（有効期限7日）ポーリング間隔。使い続けている限り
 * ログアウトされないよう、期限より十分短い間隔で`/auth/me`を呼び新しいトークンへ
 * 差し替える（タブを開いたままにしている間だけ効く。閉じて7日超放置すれば
 * 再ログインが必要になるのは意図どおり）。
 */
const TOKEN_REFRESH_INTERVAL_MS = 6 * 60 * 60 * 1000;

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
  /**
   * `preserveRedirect: false`（既定は`true`）を指定すると、ログアウトを検知した
   * `RequireAuth`が`/login`へリダイレクトする際に`?redirect=`を付与しない
   * （ホームへ戻したい設定画面の「ログアウト」ボタン等の明示的操作向け）。
   * トークン失効（401）・スライディング延命失敗による自動ログアウトは
   * 既定どおり`?redirect=`を残し、再ログイン後に元の画面へ戻れるようにする。
   */
  logout: (opts?: { preserveRedirect?: boolean }) => void;
  /** `logout({ preserveRedirect: false })`直後の1回だけ`true`。`RequireAuth`が参照する。 */
  suppressLoginRedirect: boolean;
}

const AuthContext = createContext<AuthContextValue>({
  user: null,
  loading: true,
  login: () => {},
  logout: () => {},
  suppressLoginRedirect: false,
});

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [loading, setLoading] = useState(true);
  const [suppressLoginRedirect, setSuppressLoginRedirect] = useState(false);
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
        localStorage.setItem("seiran_token", result.user.token);
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
    setSuppressLoginRedirect(false);
    localStorage.setItem("seiran_token", token);
    setUser(user);
    applyLanguagePreference(user);
  }

  function logout(opts?: { preserveRedirect?: boolean }) {
    setSuppressLoginRedirect(opts?.preserveRedirect === false);
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
            localStorage.setItem("seiran_token", result.user.token);
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

  // JWTのスライディング延命: タブを開いたまま使い続けている限り、7日の有効期限が
  // 切れる前に定期的に新しいトークンへ差し替える。ログインしていない間は
  // `/auth/me`を呼ばない（getTokenで都度確認する。userステートを依存配列に
  // 入れるとリフレッシュのたびにuserオブジェクトが新しくなりintervalが
  // 張り直されてしまうため、mount時に一度だけ張る）。
  useEffect(() => {
    const interval = setInterval(() => {
      if (!getToken()) return;
      void resolveSession(() => api.auth.me()).then((result) => {
        if (result.kind === "authenticated") {
          localStorage.setItem("seiran_token", result.user.token);
        } else if (result.kind === "expired") {
          logout();
          navigate("/login", { replace: true });
        }
      });
    }, TOKEN_REFRESH_INTERVAL_MS);
    return () => clearInterval(interval);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <AuthContext.Provider value={{ user, loading, login, logout, suppressLoginRedirect }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  return useContext(AuthContext);
}
