import { FormEvent, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, FediverseRelay, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

export default function RelaysPanel() {
  const { t } = useTranslation();
  const [relays, setRelays] = useState<FediverseRelay[]>([]);
  const [inboxUrl, setInboxUrl] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const load = () => {
    setLoading(true);
    api.admin
      .listRelays()
      .then(setRelays)
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  };

  useEffect(load, []);

  async function addRelay(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const relay = await api.admin.createRelay(inboxUrl.trim());
      setRelays((current) => [...current, relay]);
      setInboxUrl("");
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function removeRelay(relay: FediverseRelay) {
    if (!confirm(t("admin:relaysPanel.deleteConfirm", { url: relay.inbox_url }))) return;
    setBusy(true);
    setError("");
    try {
      await api.admin.deleteRelay(relay.id);
      setRelays((current) => current.filter((item) => item.id !== relay.id));
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:relaysPanel.title")}</h2>
      <p className={styles.hint}>{t("admin:relaysPanel.description")}</p>
      {error && <p className={styles.error}>{error}</p>}
      <form className={styles.card} onSubmit={addRelay}>
        <label className={styles.label}>
          {t("admin:relaysPanel.inboxUrlLabel")}
          <input
            className={styles.input}
            type="url"
            required
            value={inboxUrl}
            onChange={(e) => setInboxUrl(e.target.value)}
            placeholder="https://relay.example/inbox"
          />
        </label>
        <button className={styles.btnPrimary} disabled={busy || !inboxUrl.trim()}>
          {t("admin:relaysPanel.addButton")}
        </button>
      </form>
      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : relays.length === 0 ? (
        <p className={panel.message}>{t("admin:relaysPanel.emptyMessage")}</p>
      ) : (
        relays.map((relay) => (
          <div className={styles.card} key={relay.id}>
            <strong>{relay.inbox_url}</strong>
            <p className={styles.hint}>
              {t(`admin:relaysPanel.status.${relay.status}`)}
              {relay.last_error ? ` — ${relay.last_error}` : ""}
            </p>
            <button className={styles.btnGhost} disabled={busy} onClick={() => removeRelay(relay)}>
              {t("common:delete")}
            </button>
          </div>
        ))
      )}
    </div>
  );
}
