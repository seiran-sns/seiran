import { createContext, useContext, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, User, getToken, setUnauthorizedHandler } from "../api/client";

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
    if (getToken()) {
      api.auth
        .me()
        .then(setUser)
        .catch(() => localStorage.removeItem("seiran_token"))
        .finally(() => setLoading(false));
    } else {
      setLoading(false);
    }
  }, []);

  function login(token: string, user: User) {
    localStorage.setItem("seiran_token", token);
    setUser(user);
  }

  function logout() {
    localStorage.removeItem("seiran_token");
    setUser(null);
  }

  // トークン失効時（401）にログイン画面へ誘導する共通処理。
  // ログイン試行自体の401（認証情報間違い）はトークン未保持のため client.ts 側で発火しない。
  useEffect(() => {
    setUnauthorizedHandler(() => {
      logout();
      navigate("/login", { replace: true });
    });
    return () => setUnauthorizedHandler(null);
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
