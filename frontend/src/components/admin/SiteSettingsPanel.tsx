import { FormEvent, useEffect, useState } from "react";
import { api, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

export default function SiteSettingsPanel() {
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [passwordSet, setPasswordSet] = useState(false);

  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [from, setFrom] = useState("");
  const [requireVerify, setRequireVerify] = useState(false);

  useEffect(() => {
    api.admin
      .getSiteSettings()
      .then((s) => {
        setHost(s.smtp_host);
        setPort(s.smtp_port);
        setUsername(s.smtp_username);
        setFrom(s.smtp_from);
        setPasswordSet(s.smtp_password_set);
        setRequireVerify(s.require_email_verification === "true");
      })
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, []);

  async function save(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      const patch: Record<string, string> = {
        smtp_host: host,
        smtp_port: port,
        smtp_username: username,
        smtp_from: from,
        require_email_verification: requireVerify ? "true" : "false",
      };
      // パスワードは入力があったときだけ送る（未入力なら既存値を維持）。
      if (password) patch.smtp_password = password;
      const s = await api.admin.updateSiteSettings(patch);
      setPasswordSet(s.smtp_password_set);
      setPassword("");
      setSaved(true);
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  if (loading) return <p className={panel.message}>読み込み中...</p>;

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>サイト設定（SMTP・登録）</h2>
      {error && <p className={styles.error}>{error}</p>}
      {saved && <p className={styles.success}>保存しました。</p>}
      <form className={styles.card} onSubmit={save}>
        <label className={styles.label}>
          SMTP ホスト
          <input className={styles.input} value={host} onChange={(e) => setHost(e.target.value)} placeholder="smtp.resend.com" />
        </label>
        <label className={styles.label}>
          SMTP ポート
          <input className={styles.input} value={port} onChange={(e) => setPort(e.target.value)} placeholder="587" />
        </label>
        <label className={styles.label}>
          SMTP ユーザー名
          <input className={styles.input} value={username} onChange={(e) => setUsername(e.target.value)} />
        </label>
        <label className={styles.label}>
          SMTP パスワード
          <input
            className={styles.input}
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder={passwordSet ? "設定済み（変更する場合のみ入力）" : "未設定"}
          />
        </label>
        <label className={styles.label}>
          差出人アドレス（From）
          <input className={styles.input} value={from} onChange={(e) => setFrom(e.target.value)} placeholder="info@seiran.org" />
        </label>
        <label className={`${styles.label}`} style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
          <input type="checkbox" checked={requireVerify} onChange={(e) => setRequireVerify(e.target.checked)} />
          新規登録時にメール確認を必須にする
        </label>
        <p className={styles.hint}>
          メール確認を必須にする場合は SMTP 設定が完了している必要があります。
        </p>
        <button className={styles.btn} type="submit" disabled={saving}>
          {saving ? "保存中..." : "保存"}
        </button>
      </form>
    </div>
  );
}
