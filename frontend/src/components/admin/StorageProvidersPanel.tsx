import { FormEvent, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, StorageProvider, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

const EMPTY = {
  name: "",
  endpoint: "",
  bucket: "",
  region: "auto",
  access_key: "",
  secret_key: "",
  public_url: "",
  capacity_mb: "",
};

export default function StorageProvidersPanel() {
  const { t } = useTranslation();
  const [providers, setProviders] = useState<StorageProvider[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<number | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState({ ...EMPTY });
  const [creating, setCreating] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editForm, setEditForm] = useState({ ...EMPTY });
  const [saving, setSaving] = useState(false);

  function load() {
    setLoading(true);
    setError("");
    api.admin
      .listStorageProviders()
      .then(setProviders)
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }

  useEffect(load, []);

  async function toggleActive(p: StorageProvider) {
    setBusyId(p.id);
    setError("");
    try {
      await api.admin.updateStorageProvider(p.id, { is_active: !p.is_active });
      load();
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  function startEdit(p: StorageProvider) {
    setError("");
    setEditingId(p.id);
    setEditForm({
      name: p.name,
      endpoint: p.endpoint,
      bucket: p.bucket,
      region: p.region,
      access_key: p.access_key,
      secret_key: "",
      public_url: p.public_url,
      capacity_mb: p.capacity_mb != null ? String(p.capacity_mb) : "",
    });
  }

  function cancelEdit() {
    setEditingId(null);
  }

  async function saveEdit(e: FormEvent, p: StorageProvider) {
    e.preventDefault();
    setSaving(true);
    setError("");
    try {
      const patch: Record<string, unknown> = {
        name: editForm.name,
        endpoint: editForm.endpoint,
        bucket: editForm.bucket,
        region: editForm.region || "auto",
        access_key: editForm.access_key,
        public_url: editForm.public_url,
        capacity_mb: editForm.capacity_mb ? Number(editForm.capacity_mb) : null,
      };
      if (editForm.secret_key) {
        patch.secret_key = editForm.secret_key;
      }
      await api.admin.updateStorageProvider(p.id, patch);
      setEditingId(null);
      load();
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  async function remove(p: StorageProvider) {
    if (!confirm(t("admin:storageProvidersPanel.deleteConfirm", { name: p.name }))) return;
    setBusyId(p.id);
    setError("");
    try {
      await api.admin.deleteStorageProvider(p.id);
      load();
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  async function create(e: FormEvent) {
    e.preventDefault();
    setCreating(true);
    setError("");
    try {
      await api.admin.createStorageProvider({
        name: form.name,
        endpoint: form.endpoint,
        bucket: form.bucket,
        region: form.region || "auto",
        access_key: form.access_key,
        secret_key: form.secret_key,
        public_url: form.public_url,
        capacity_mb: form.capacity_mb ? Number(form.capacity_mb) : null,
      });
      setForm({ ...EMPTY });
      setShowForm(false);
      load();
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setCreating(false);
    }
  }

  const set = (k: keyof typeof EMPTY) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setForm((f) => ({ ...f, [k]: e.target.value }));

  const setEdit = (k: keyof typeof EMPTY) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setEditForm((f) => ({ ...f, [k]: e.target.value }));

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:storageProvidersPanel.title")}</h2>
      {error && <p className={styles.error}>{error}</p>}

      <div className={styles.card}>
        {providers.length === 0 && <p className={panel.message}>{t("admin:storageProvidersPanel.emptyMessage")}</p>}
        {providers.map((p) => (
          <div key={p.id}>
            <div className={styles.row}>
              <div className={styles.grow}>
                <div className={styles.primaryText}>{p.name}</div>
                <div className={styles.subText}>
                  {p.endpoint} / {p.bucket}
                  {p.capacity_mb != null && t("admin:storageProvidersPanel.capacitySuffix", { capacity: p.capacity_mb })}
                </div>
              </div>
              <span className={`${styles.badge} ${p.is_active ? styles.badgeAdmin : ""}`}>
                {p.is_active ? t("admin:storageProvidersPanel.active") : t("admin:storageProvidersPanel.inactive")}
              </span>
              <button
                className={styles.btnGhost}
                disabled={busyId === p.id}
                onClick={() => (editingId === p.id ? cancelEdit() : startEdit(p))}
              >
                {editingId === p.id ? t("common:cancel") : t("admin:storageProvidersPanel.editButton")}
              </button>
              <button className={styles.btnGhost} disabled={busyId === p.id} onClick={() => toggleActive(p)}>
                {p.is_active ? t("admin:storageProvidersPanel.deactivateButton") : t("admin:storageProvidersPanel.activateButton")}
              </button>
              <button className={styles.btnDanger} disabled={busyId === p.id} onClick={() => remove(p)}>
                {t("common:delete")}
              </button>
            </div>

            {editingId === p.id && (
              <form className={styles.card} onSubmit={(e) => saveEdit(e, p)}>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.nameLabel")}
                  <input className={styles.input} value={editForm.name} onChange={setEdit("name")} required />
                </label>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.endpointLabel")}
                  <input className={styles.input} value={editForm.endpoint} onChange={setEdit("endpoint")} required />
                </label>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.bucketLabel")}
                  <input className={styles.input} value={editForm.bucket} onChange={setEdit("bucket")} required />
                </label>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.regionLabel")}
                  <input className={styles.input} value={editForm.region} onChange={setEdit("region")} placeholder="auto" />
                </label>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.accessKeyLabel")}
                  <input className={styles.input} value={editForm.access_key} onChange={setEdit("access_key")} required />
                </label>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.secretKeyLabel")}
                  <input
                    className={styles.input}
                    type="password"
                    value={editForm.secret_key}
                    onChange={setEdit("secret_key")}
                    placeholder={t("admin:storageProvidersPanel.secretKeyKeepPlaceholder")}
                  />
                </label>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.publicUrlLabel")}
                  <input className={styles.input} value={editForm.public_url} onChange={setEdit("public_url")} required />
                </label>
                <label className={styles.label}>
                  {t("admin:storageProvidersPanel.capacityLabel")}
                  <input className={styles.input} type="number" value={editForm.capacity_mb} onChange={setEdit("capacity_mb")} />
                </label>
                <div className={styles.actions}>
                  <button className={styles.btn} type="submit" disabled={saving}>
                    {saving ? t("admin:storageProvidersPanel.saving") : t("common:save")}
                  </button>
                  <button className={styles.btnGhost} type="button" onClick={cancelEdit}>
                    {t("common:cancel")}
                  </button>
                </div>
              </form>
            )}
          </div>
        ))}
      </div>

      {showForm ? (
        <form className={styles.card} onSubmit={create}>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.nameLabel")}
            <input className={styles.input} value={form.name} onChange={set("name")} required />
          </label>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.endpointLabel")}
            <input className={styles.input} value={form.endpoint} onChange={set("endpoint")} placeholder="https://xxx.r2.cloudflarestorage.com" required />
          </label>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.bucketLabel")}
            <input className={styles.input} value={form.bucket} onChange={set("bucket")} required />
          </label>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.regionLabel")}
            <input className={styles.input} value={form.region} onChange={set("region")} placeholder="auto" />
          </label>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.accessKeyLabel")}
            <input className={styles.input} value={form.access_key} onChange={set("access_key")} required />
          </label>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.secretKeyLabel")}
            <input className={styles.input} type="password" value={form.secret_key} onChange={set("secret_key")} required />
          </label>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.publicUrlLabel")}
            <input className={styles.input} value={form.public_url} onChange={set("public_url")} placeholder="https://media.example.com" required />
          </label>
          <label className={styles.label}>
            {t("admin:storageProvidersPanel.capacityLabel")}
            <input className={styles.input} type="number" value={form.capacity_mb} onChange={set("capacity_mb")} />
          </label>
          <div className={styles.actions}>
            <button className={styles.btn} type="submit" disabled={creating}>
              {creating ? t("admin:storageProvidersPanel.creating") : t("common:create")}
            </button>
            <button className={styles.btnGhost} type="button" onClick={() => setShowForm(false)}>
              {t("common:cancel")}
            </button>
          </div>
        </form>
      ) : (
        <button className={styles.btn} onClick={() => setShowForm(true)}>
          {t("admin:storageProvidersPanel.addButton")}
        </button>
      )}
    </div>
  );
}
