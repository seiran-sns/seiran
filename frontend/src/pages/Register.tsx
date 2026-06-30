import { FormEvent, useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import styles from "./Auth.module.css";

export default function Register() {
  const [email, setEmail] = useState("");
  const [sent, setSent] = useState(false);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      await api.auth.requestEmailVerification(email);
      setSent(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "送信に失敗しました");
    } finally {
      setLoading(false);
    }
  }

  if (sent) {
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

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>seiran</h1>
        <h2 className={styles.subtitle}>新規登録</h2>
        <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
          まずメールアドレスを入力してください。確認メールを送信します。
        </p>
        <form onSubmit={handleSubmit} className={styles.form}>
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
