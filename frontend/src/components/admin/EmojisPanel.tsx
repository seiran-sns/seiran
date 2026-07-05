import { ChangeEvent, FormEvent, useEffect, useRef, useState } from "react";
import { api, CustomEmoji, DriveFile, EmojiImportJob, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

export default function EmojisPanel() {
  const [emojis, setEmojis] = useState<CustomEmoji[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);

  const [shortcode, setShortcode] = useState("");
  const [category, setCategory] = useState("");
  const [tags, setTags] = useState("");
  const [uploaded, setUploaded] = useState<DriveFile | null>(null);
  const [uploading, setUploading] = useState(false);
  const [creating, setCreating] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  // タグのインライン編集（#49）
  const [editId, setEditId] = useState<string | null>(null);
  const [editTags, setEditTags] = useState("");

  // Misskey ZIP インポート（#50）
  const importFileRef = useRef<HTMLInputElement>(null);
  const [importing, setImporting] = useState(false);
  const [importJob, setImportJob] = useState<EmojiImportJob | null>(null);
  const [importError, setImportError] = useState("");

  /** 空白・カンマ区切りの文字列をタグ配列に変換する（重複・空要素はバックエンドでも正規化）。 */
  function parseTags(s: string): string[] {
    return s
      .split(/[\s,]+/)
      .map((t) => t.trim())
      .filter(Boolean);
  }

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
        tags: parseTags(tags),
      });
      setShortcode("");
      setCategory("");
      setTags("");
      setUploaded(null);
      load();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setCreating(false);
    }
  }

  async function onImportFile(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setImporting(true);
    setImportError("");
    setImportJob(null);
    let job: EmojiImportJob;
    try {
      job = await api.admin.importEmojis(file);
      setImportJob(job);
    } catch (err) {
      setImportError(getErrorMessage(err));
      setImporting(false);
      return;
    }
    // ポーリングして進捗を更新
    const poll = setInterval(async () => {
      try {
        const status = await api.admin.getEmojiImportStatus(job.jobId);
        setImportJob(status);
        if (status.done) {
          clearInterval(poll);
          setImporting(false);
          load();
        }
      } catch {
        clearInterval(poll);
        setImporting(false);
      }
    }, 1500);
  }

  function startEdit(em: CustomEmoji) {
    setEditId(em.id);
    setEditTags(em.tags.join(" "));
    setError("");
  }

  async function saveTags(em: CustomEmoji) {
    setBusyId(em.id);
    setError("");
    try {
      await api.admin.updateEmoji(em.id, { tags: parseTags(editTags) });
      setEditId(null);
      load();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusyId(null);
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

      {/* Misskey ZIP インポート（#50） */}
      <div className={styles.card}>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Misskey ZIP インポート</div>
        <input
          ref={importFileRef}
          type="file"
          accept=".zip"
          style={{ display: "none" }}
          onChange={onImportFile}
        />
        <div className={styles.actions}>
          <button
            type="button"
            className={styles.btnGhost}
            onClick={() => importFileRef.current?.click()}
            disabled={importing}
          >
            {importing ? "インポート中..." : "ZIPを選択してインポート"}
          </button>
        </div>
        {importError && <p className={styles.error} style={{ marginTop: 8 }}>{importError}</p>}
        {importJob && (
          <div style={{ marginTop: 8, fontSize: 13, color: "var(--color-text-sub, #666)" }}>
            {importJob.done ? "完了" : "処理中"} — 追加: {importJob.processed} / スキップ: {importJob.skipped} / 失敗: {importJob.failed} / 合計: {importJob.total}
            {importJob.errors.length > 0 && (
              <ul style={{ margin: "4px 0 0", paddingLeft: 16 }}>
                {importJob.errors.slice(0, 10).map((err, i) => (
                  <li key={i} style={{ color: "var(--color-danger, #c00)" }}>{err}</li>
                ))}
                {importJob.errors.length > 10 && <li>… 他 {importJob.errors.length - 10} 件</li>}
              </ul>
            )}
          </div>
        )}
      </div>

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
        <label className={styles.label}>
          タグ（任意・空白またはカンマ区切り。ピッカーの部分一致対象）
          <input
            className={styles.input}
            value={tags}
            onChange={(e) => setTags(e.target.value)}
            placeholder="猫 かわいい blob-cat"
          />
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
              {editId === em.id ? (
                <div className={styles.actions} style={{ marginTop: 6 }}>
                  <input
                    className={styles.input}
                    value={editTags}
                    onChange={(e) => setEditTags(e.target.value)}
                    placeholder="空白またはカンマ区切り"
                    style={{ flex: 1 }}
                  />
                  <button className={styles.btn} disabled={busyId === em.id} onClick={() => saveTags(em)}>
                    保存
                  </button>
                  <button className={styles.btnGhost} onClick={() => setEditId(null)}>
                    取消
                  </button>
                </div>
              ) : (
                em.tags.length > 0 && (
                  <div className={styles.subText}>🏷 {em.tags.join(" / ")}</div>
                )
              )}
            </div>
            {editId !== em.id && (
              <button className={styles.btnGhost} disabled={busyId === em.id} onClick={() => startEdit(em)}>
                タグ編集
              </button>
            )}
            <button className={styles.btnDanger} disabled={busyId === em.id} onClick={() => remove(em)}>
              削除
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
