import { ChangeEvent, FormEvent, useEffect, useRef, useState } from "react";
import { api, CustomEmoji, DriveFile, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

export default function EmojisPanel() {
  const [emojis, setEmojis] = useState<CustomEmoji[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);

  const [shortcode, setShortcode] = useState("");
  const [category, setCategory] = useState("");
  const [uploaded, setUploaded] = useState<DriveFile | null>(null);
  const [uploading, setUploading] = useState(false);
  const [creating, setCreating] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  function load() {
    setLoading(true);
    setError("");
    api.admin
      .listEmojis()
      .then(setEmojis)
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }

  useEffect(load, []);

  async function onFile(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setUploading(true);
    setError("");
    try {
      setUploaded(await api.media.upload(file, "emoji"));
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setUploading(false);
    }
  }

  async function create(e: FormEvent) {
    e.preventDefault();
    if (!uploaded || !shortcode.trim()) return;
    setCreating(true);
    setError("");
    try {
      await api.admin.createEmoji({
        shortcode: shortcode.trim(),
        media_file_id: Number(uploaded.id),
        category: category.trim() || undefined,
      });
      setShortcode("");
      setCategory("");
      setUploaded(null);
      load();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setCreating(false);
    }
  }

  async function remove(em: CustomEmoji) {
    if (!confirm(`絵文字 :${em.shortcode}: を削除しますか？`)) return;
    setBusyId(em.id);
    setError("");
    try {
      await api.admin.deleteEmoji(em.id);
      load();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusyId(null);
    }
  }

  if (loading) return <p className={panel.message}>読み込み中...</p>;

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>カスタム絵文字</h2>
      {error && <p className={styles.error}>{error}</p>}

      <form className={styles.card} onSubmit={create}>
        <div className={styles.actions} style={{ marginBottom: 12 }}>
          <input ref={fileRef} type="file" accept="image/*" style={{ display: "none" }} onChange={onFile} />
          <button type="button" className={styles.btnGhost} onClick={() => fileRef.current?.click()} disabled={uploading}>
            {uploading ? "アップロード中..." : uploaded ? "画像を変更" : "画像を選択"}
          </button>
          {uploaded && <img src={uploaded.url} alt="" style={{ height: 32, borderRadius: 4 }} />}
        </div>
        <label className={styles.label}>
          ショートコード（英数字・アンダースコア）
          <input
            className={styles.input}
            value={shortcode}
            onChange={(e) => setShortcode(e.target.value)}
            placeholder="party_parrot"
            pattern="[A-Za-z0-9_]+"
            required
          />
        </label>
        <label className={styles.label}>
          カテゴリ（任意）
          <input className={styles.input} value={category} onChange={(e) => setCategory(e.target.value)} />
        </label>
        <button className={styles.btn} type="submit" disabled={creating || !uploaded || !shortcode.trim()}>
          {creating ? "追加中..." : "絵文字を追加"}
        </button>
      </form>

      <div className={styles.card}>
        {emojis.length === 0 && <p className={panel.message}>カスタム絵文字がありません。</p>}
        {emojis.map((em) => (
          <div key={em.id} className={styles.row}>
            <div className={styles.grow}>
              <div className={styles.primaryText}>:{em.shortcode}:</div>
              {em.category && <div className={styles.subText}>{em.category}</div>}
            </div>
            <button className={styles.btnDanger} disabled={busyId === em.id} onClick={() => remove(em)}>
              削除
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
