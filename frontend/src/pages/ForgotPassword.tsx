import { FormEvent, useState } from "react";
import { Link } from "react-router-dom";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type Phase = "form" | "sent";

export default function ForgotPassword() {
  const [phase, setPhase] = useState<Phase>("form");
  const [email, setEmail] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      await api.auth.requestPasswordReset(email);
      setPhase("sent");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  if (phase === "sent") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>seiran</h1>
          <h2 className={styles.subtitle}>メールを確認してください</h2>
          <p style={{ textAlign: "center", color: "#a0aec0", fontSize: "0.9rem", margin: "0 0 24px" }}>
            入力されたメールアドレスが登録されている場合、パスワードリセットリンクを送信しました。
            メールボックスをご確認ください。
          </p>
          <p className={styles.link}>
            <Link to="/login">ログインページへ戻る</Link>
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>seiran</h1>
        <h2 className={styles.subtitle}>パスワードをリセット</h2>
        <p className={styles.description}>
          登録済みのメールアドレスを入力してください。リセットリンクをお送りします。
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
            {loading ? "送信中..." : "リセットリンクを送信"}
          </button>
        </form>
        <p className={styles.link}>
          <Link to="/login">ログインページへ戻る</Link>
        </p>
      </div>
    </div>
  );
}
