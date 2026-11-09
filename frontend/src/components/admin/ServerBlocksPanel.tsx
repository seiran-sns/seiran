import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

/**
 * 管理画面「サーバーブロック」タブ。拒否サーバーを1行1ドメインの textarea で読み書きする
 * （サブドメインも対象。扱いはバックエンドの`seiran_common::denied_host`）。保存後は
 * バックエンドが正規化した一覧で textarea を置き換える。
 */
export default function ServerBlocksPanel() {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    api.admin
      .getDeniedHosts()
      .then((res) => setText(res.hosts.join("\n")))
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, []);

  async function save(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      const res = await api.admin.updateDeniedHosts(text);
      setText(res.hosts.join("\n"));
      setSaved(true);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  if (loading) {
    return <p className={panel.message}>{t("common:loading")}</p>;
  }

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:serverBlocksPanel.deniedTitle")}</h2>
      <p className={styles.hint}>{t("admin:serverBlocksPanel.deniedDescription")}</p>
      <form onSubmit={save}>
        <label className={styles.label}>
          {t("admin:serverBlocksPanel.deniedLabel")}
          <textarea
            className={styles.input}
            rows={12}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setSaved(false);
            }}
            placeholder={t("admin:serverBlocksPanel.deniedPlaceholder")}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            data-testid="denied-hosts-textarea"
          />
        </label>
        {error && <p className={styles.error}>{error}</p>}
        {saved && <p className={styles.success}>{t("admin:serverBlocksPanel.savedMessage")}</p>}
        <button className={styles.btn} type="submit" disabled={saving}>
          {saving ? t("admin:serverBlocksPanel.saving") : t("admin:serverBlocksPanel.saveButton")}
        </button>
      </form>
    </div>
  );
}
