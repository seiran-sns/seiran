import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type Phase =
  | { kind: "verifying" }
  | { kind: "form"; token: string }
  | { kind: "invalid"; message: string }
  | { kind: "done" };

export default function ResetPassword() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const [phase, setPhase] = useState<Phase>({ kind: "verifying" });
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // マウント時にトークンを検証する
  useEffect(() => {
    const token = searchParams.get("token");
    if (!token) {
      setPhase({ kind: "invalid", message: "URLが無効です" });
      return;
    }
    const controller = new AbortController();
    api.auth.verifyResetToken(token, controller.signal)
      .then(() => setPhase({ kind: "form", token }))
      .catch((err) => {
        if (controller.signal.aborted) return;
        setPhase({ kind: "invalid", message: getErrorMessage(err) });
      });
    return () => controller.abort();
  }, [searchParams]);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (phase.kind !== "form") return;
    setError("");

    if (password.length < 8) {
      setError("パスワードは8文字以上で入力してください");
      return;
    }
    if (password !== passwordConfirm) {
      setError("パスワードが一致しません");
      return;
    }

    setSubmitting(true);
    try {
      await api.auth.resetPassword(phase.token, password);
      navigate("/login");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  if (phase.kind === "verifying") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>seiran</h1>
          <p style={{ textAlign: "center", color: "#a0aec0" }}>リンクを確認中...</p>
        </div>
      </div>
    );
  }

  if (phase.kind === "invalid") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>seiran</h1>
          <h2 className={styles.subtitle}>リンクが無効です</h2>
          <p className={styles.error} style={{ textAlign: "center" }}>
            {phase.message || "リンクが無効または期限切れです"}
          </p>
          <p className={styles.link} style={{ marginTop: "1rem" }}>
            <Link to="/forgot-password">パスワードリセットをやり直す</Link>
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>seiran</h1>
        <h2 className={styles.subtitle}>新しいパスワードを設定</h2>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            新しいパスワード（8文字以上）
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={styles.input}
              required
              minLength={8}
              autoFocus
            />
          </label>
          <label className={styles.label}>
            パスワード（確認）
            <input
              type="password"
              value={passwordConfirm}
              onChange={(e) => setPasswordConfirm(e.target.value)}
              className={styles.input}
              required
              minLength={8}
            />
          </label>
          {error && <p className={styles.error}>{error}</p>}
          <button type="submit" className={styles.button} disabled={submitting}>
            {submitting ? "更新中..." : "パスワードを更新"}
          </button>
        </form>
        <p className={styles.link}>
          <Link to="/login">ログインページへ戻る</Link>
        </p>
      </div>
    </div>
  );
}
