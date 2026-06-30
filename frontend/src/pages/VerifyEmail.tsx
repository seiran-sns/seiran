import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { api } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Auth.module.css";

type State =
  | { phase: "verifying" }
  | { phase: "form"; registrationToken: string }
  | { phase: "error"; message: string };

export default function VerifyEmail() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { login } = useAuth();

  const [state, setState] = useState<State>({ phase: "verifying" });
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [formError, setFormError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // マウント時にトークンを検証する
  useEffect(() => {
    const token = searchParams.get("token");
    if (!token) {
      setState({ phase: "error", message: "URLが無効です" });
      return;
    }
    api.auth.verifyEmailToken(token)
      .then((res) => setState({ phase: "form", registrationToken: res.registration_token }))
      .catch((err) => setState({
        phase: "error",
        message: err instanceof Error ? err.message : "トークンが無効か期限切れです",
      }));
  }, [searchParams]);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (state.phase !== "form") return;
    setFormError("");
    if (password.length < 8) {
      setFormError("パスワードは8文字以上で入力してください");
      return;
    }
    setSubmitting(true);
    try {
      const res = await api.auth.register(username, password, state.registrationToken);
      login(res.token, res.user);
      navigate("/");
    } catch (err) {
      setFormError(err instanceof Error ? err.message : "登録に失敗しました");
    } finally {
      setSubmitting(false);
    }
  }

  if (state.phase === "verifying") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>seiran</h1>
          <p style={{ textAlign: "center", color: "#a0aec0" }}>メールアドレスを確認中...</p>
        </div>
      </div>
    );
  }

  if (state.phase === "error") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>seiran</h1>
          <h2 className={styles.subtitle}>確認に失敗しました</h2>
          <p className={styles.error} style={{ textAlign: "center" }}>{state.message}</p>
          <p className={styles.link} style={{ marginTop: "1rem" }}>
            <Link to="/register">最初からやり直す</Link>
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>seiran</h1>
        <h2 className={styles.subtitle}>アカウント情報を設定</h2>
        <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
          メールアドレスを確認しました。ユーザー名とパスワードを設定してください。
        </p>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            ユーザー名
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              className={styles.input}
              required
              autoFocus
              pattern="[a-zA-Z0-9_]+"
              title="英数字とアンダースコアのみ使用できます"
            />
          </label>
          <label className={styles.label}>
            パスワード（8文字以上）
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={styles.input}
              required
              minLength={8}
            />
          </label>
          {formError && <p className={styles.error}>{formError}</p>}
          <button type="submit" className={styles.button} disabled={submitting}>
            {submitting ? "登録中..." : "登録する"}
          </button>
        </form>
      </div>
    </div>
  );
}
