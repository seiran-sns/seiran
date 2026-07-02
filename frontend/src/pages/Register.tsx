import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

export default function Register() {
  const navigate = useNavigate();

  // requireEmailVerification フラグ（null = まだロード中）
  const [requireEmailVerification, setRequireEmailVerification] = useState<boolean | null>(null);

  // メール確認フロー用
  const [email, setEmail] = useState("");
  const [sent, setSent] = useState(false);

  // 直接登録フロー用
  const [directEmail, setDirectEmail] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    api.meta().then((meta) => {
      setRequireEmailVerification(meta.requireEmailVerification ?? false);
    }).catch(() => {
      // メタ取得失敗時はデフォルト false
      setRequireEmailVerification(false);
    });
  }, []);

  // ─── メール確認フロー ────────────────────────────────────────────

  async function handleVerifySubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      await api.auth.requestEmailVerification(email);
      setSent(true);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  if (requireEmailVerification === true && sent) {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>seiran</h1>
          <h2 className={styles.subtitle}>メールを送信しました</h2>
          <p style={{ textAlign: "center", color: "#a0aec0", lineHeight: 1.6 }}>
            <strong>{email}</strong> に確認メールを送りました。<br />
            メール内のリンクをクリックして登録を完了してください。
          </p>
          <p className={styles.link} style={{ marginTop: "1.5rem" }}>
            <Link to="/login">ログインページへ</Link>
          </p>
        </div>
      </div>
    );
  }

  if (requireEmailVerification === true) {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>seiran</h1>
          <h2 className={styles.subtitle}>新規登録</h2>
          <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
            まずメールアドレスを入力してください。確認メールを送信します。
          </p>
          <form onSubmit={handleVerifySubmit} className={styles.form}>
            <label className={styles.label}>
              メールアドレス
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                className={styles.input}
                required
                autoFocus
              />
            </label>
            {error && <p className={styles.error}>{error}</p>}
            <button type="submit" className={styles.button} disabled={loading}>
              {loading ? "送信中..." : "確認メールを送る"}
            </button>
          </form>
          <p className={styles.link}>
            すでにアカウントをお持ちの方は <Link to="/login">ログイン</Link>
          </p>
        </div>
      </div>
    );
  }

  // ─── 直接登録フロー（requireEmailVerification === false） ─────────

  async function handleDirectSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.auth.registerDirect(directEmail, username, password);
      localStorage.setItem("seiran_token", res.token);
      navigate("/");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  // ロード中は空を表示
  if (requireEmailVerification === null) {
    return null;
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>seiran</h1>
        <h2 className={styles.subtitle}>新規登録</h2>
        <form onSubmit={handleDirectSubmit} className={styles.form}>
          <label className={styles.label}>
            メールアドレス
            <input
              type="email"
              value={directEmail}
              onChange={(e) => setDirectEmail(e.target.value)}
              className={styles.input}
              required
              autoFocus
            />
          </label>
          <label className={styles.label}>
            ユーザー名
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              className={styles.input}
              required
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
            />
          </label>
          {error && <p className={styles.error}>{error}</p>}
          <button type="submit" className={styles.button} disabled={loading}>
            {loading ? "登録中..." : "登録する"}
          </button>
        </form>
        <p className={styles.link}>
          すでにアカウントをお持ちの方は <Link to="/login">ログイン</Link>
        </p>
      </div>
    </div>
  );
}
