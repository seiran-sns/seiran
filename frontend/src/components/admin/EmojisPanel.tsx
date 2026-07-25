import { ChangeEvent, FormEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, CustomEmoji, DriveFile, EmojiImportJob, RemoteEmoji, getErrorMessage } from "../../api/client";
import { mediaUrl } from "../../utils/mediaProxy";
import EmojiImportDialog from "./EmojiImportDialog";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

type EmojiTab = "local" | "remote";

export default function EmojisPanel() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<EmojiTab>("local");
  const [emojis, setEmojis] = useState<CustomEmoji[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [keyword, setKeyword] = useState("");

  // リモート絵文字（#73）
  const [remoteEmojis, setRemoteEmojis] = useState<RemoteEmoji[]>([]);
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [remoteError, setRemoteError] = useState("");
  const [remoteKeyword, setRemoteKeyword] = useState("");
  const [importTarget, setImportTarget] = useState<RemoteEmoji | null>(null);

  const [shortcode, setShortcode] = useState("");
  const [category, setCategory] = useState("");
  const [tags, setTags] = useState("");
  const [license, setLicense] = useState("");
  const [uploaded, setUploaded] = useState<DriveFile | null>(null);
  const [uploading, setUploading] = useState(false);
  const [creating, setCreating] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  // タグ・ライセンスのインライン編集（#49, #63）
  const [editId, setEditId] = useState<string | null>(null);
  const [editTags, setEditTags] = useState("");
  const [editLicense, setEditLicense] = useState("");

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

  function loadRemote(kw: string) {
    setRemoteLoading(true);
    setRemoteError("");
    api.admin
      .listRemoteEmojis(kw)
      .then(setRemoteEmojis)
      .catch((e) => setRemoteError(getErrorMessage(e)))
      .finally(() => setRemoteLoading(false));
  }

  // リモートタブを開いたとき、およびキーワード入力の300msデバウンスで再取得する。
  useEffect(() => {
    if (tab !== "remote") return;
    const timer = setTimeout(() => loadRemote(remoteKeyword), 300);
    return () => clearTimeout(timer);
  }, [tab, remoteKeyword]);

  const filteredEmojis = emojis.filter((em) => {
    const kw = keyword.trim().toLowerCase();
    if (!kw) return true;
    return em.shortcode.toLowerCase().includes(kw) || em.tags.some((tag) => tag.toLowerCase().includes(kw));
  });

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
        media_file_id: uploaded.id,
        category: category.trim() || undefined,
        tags: parseTags(tags),
        license: license.trim() || undefined,
      });
      setShortcode("");
      setCategory("");
      setTags("");
      setLicense("");
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
    setEditLicense(em.license ?? "");
    setError("");
  }

  async function saveEdit(em: CustomEmoji) {
    setBusyId(em.id);
    setError("");
    try {
      await api.admin.updateEmoji(em.id, { tags: parseTags(editTags), license: editLicense.trim() });
      setEditId(null);
      load();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusyId(null);
    }
  }

  async function remove(em: CustomEmoji) {
    if (!confirm(t("admin:emojisPanel.deleteConfirm", { shortcode: em.shortcode }))) return;
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

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:emojisPanel.title")}</h2>

      <div className={styles.actions} style={{ marginBottom: 12 }}>
        <button
          type="button"
          className={tab === "local" ? styles.btn : styles.btnGhost}
          onClick={() => setTab("local")}
        >
          {t("admin:emojisPanel.localTab")}
        </button>
        <button
          type="button"
          className={tab === "remote" ? styles.btn : styles.btnGhost}
          onClick={() => setTab("remote")}
        >
          {t("admin:emojisPanel.remoteTab")}
        </button>
      </div>

      {tab === "local" ? (
        <>
          {error && <p className={styles.error}>{error}</p>}

          {/* Misskey ZIP インポート（#50） */}
          <div className={styles.card}>
            <div style={{ fontWeight: 600, marginBottom: 8 }}>{t("admin:emojisPanel.importTitle")}</div>
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
                {importing ? t("admin:emojisPanel.importing") : t("admin:emojisPanel.selectZipButton")}
              </button>
            </div>
            {importError && <p className={styles.error} style={{ marginTop: 8 }}>{importError}</p>}
            {importJob && (
              <div style={{ marginTop: 8, fontSize: 13, color: "var(--color-text-sub, #666)" }}>
                {importJob.done ? t("admin:emojisPanel.importDone") : t("admin:emojisPanel.importProcessing")} —{" "}
                {t("admin:emojisPanel.importStats", {
                  processed: importJob.processed,
                  skipped: importJob.skipped,
                  failed: importJob.failed,
                  total: importJob.total,
                })}
                {importJob.errors.length > 0 && (
                  <ul style={{ margin: "4px 0 0", paddingLeft: 16 }}>
                    {importJob.errors.slice(0, 10).map((err, i) => (
                      <li key={i} style={{ color: "var(--color-danger, #c00)" }}>{err}</li>
                    ))}
                    {importJob.errors.length > 10 && (
                      <li>{t("admin:emojisPanel.importMoreErrors", { count: importJob.errors.length - 10 })}</li>
                    )}
                  </ul>
                )}
              </div>
            )}
          </div>

          <form className={styles.card} onSubmit={create}>
            <div className={styles.actions} style={{ marginBottom: 12 }}>
              <input ref={fileRef} type="file" accept="image/*" style={{ display: "none" }} onChange={onFile} />
              <button type="button" className={styles.btnGhost} onClick={() => fileRef.current?.click()} disabled={uploading}>
                {uploading
                  ? t("admin:emojisPanel.uploading")
                  : uploaded
                    ? t("admin:emojisPanel.changeImageButton")
                    : t("admin:emojisPanel.selectImageButton")}
              </button>
              {uploaded && <img src={uploaded.url} alt="" style={{ height: 32, borderRadius: 4 }} />}
            </div>
            <label className={styles.label}>
              {t("admin:emojisPanel.shortcodeLabel")}
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
              {t("admin:emojisPanel.categoryLabel")}
              <input className={styles.input} value={category} onChange={(e) => setCategory(e.target.value)} />
            </label>
            <label className={styles.label}>
              {t("admin:emojisPanel.tagsLabel")}
              <input
                className={styles.input}
                value={tags}
                onChange={(e) => setTags(e.target.value)}
                placeholder={t("admin:emojisPanel.tagsPlaceholder")}
              />
            </label>
            <label className={styles.label}>
              {t("admin:emojisPanel.licenseLabel")}
              <input
                className={styles.input}
                value={license}
                onChange={(e) => setLicense(e.target.value)}
                placeholder={t("admin:emojisPanel.licensePlaceholder")}
              />
            </label>
            <button className={styles.btn} type="submit" disabled={creating || !uploaded || !shortcode.trim()}>
              {creating ? t("admin:emojisPanel.creating") : t("admin:emojisPanel.addButton")}
            </button>
          </form>

          <input
            className={styles.input}
            style={{ marginBottom: 12, width: "100%" }}
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
            placeholder={t("admin:emojisPanel.searchPlaceholder")}
          />

          <div className={styles.card}>
            {filteredEmojis.length === 0 && <p className={panel.message}>{t("admin:emojisPanel.emptyMessage")}</p>}
            {filteredEmojis.map((em) => (
              <div key={em.id} className={styles.row}>
                {em.url && <img src={em.url} alt={em.shortcode} style={{ height: 28, width: 28, objectFit: "contain" }} />}
                <div className={styles.grow}>
                  <div className={styles.primaryText}>:{em.shortcode}:</div>
                  {em.category && <div className={styles.subText}>{em.category}</div>}
                  {editId === em.id ? (
                    <div className={styles.actions} style={{ marginTop: 6, flexDirection: "column", alignItems: "stretch" }}>
                      <input
                        className={styles.input}
                        value={editTags}
                        onChange={(e) => setEditTags(e.target.value)}
                        placeholder={t("admin:emojisPanel.editTagsPlaceholder")}
                      />
                      <input
                        className={styles.input}
                        value={editLicense}
                        onChange={(e) => setEditLicense(e.target.value)}
                        placeholder={t("admin:emojisPanel.editLicensePlaceholder")}
                      />
                      <div className={styles.actions}>
                        <button className={styles.btn} disabled={busyId === em.id} onClick={() => saveEdit(em)}>
                          {t("common:save")}
                        </button>
                        <button className={styles.btnGhost} onClick={() => setEditId(null)}>
                          {t("common:cancel")}
                        </button>
                      </div>
                    </div>
                  ) : (
                    <>
                      {em.tags.length > 0 && <div className={styles.subText}>🏷 {em.tags.join(" / ")}</div>}
                      {em.license && <div className={styles.subText}>📄 {em.license}</div>}
                    </>
                  )}
                </div>
                {editId !== em.id && (
                  <button className={styles.btnGhost} disabled={busyId === em.id} onClick={() => startEdit(em)}>
                    {t("admin:emojisPanel.editButton")}
                  </button>
                )}
                <button className={styles.btnDanger} disabled={busyId === em.id} onClick={() => remove(em)}>
                  {t("common:delete")}
                </button>
              </div>
            ))}
          </div>
        </>
      ) : (
        <>
          {remoteError && <p className={styles.error}>{remoteError}</p>}
          <input
            className={styles.input}
            style={{ marginBottom: 12, width: "100%" }}
            value={remoteKeyword}
            onChange={(e) => setRemoteKeyword(e.target.value)}
            placeholder={t("admin:emojisPanel.remoteSearchPlaceholder")}
          />
          <div className={styles.card}>
            {remoteLoading ? (
              <p className={panel.message}>{t("common:loading")}</p>
            ) : remoteEmojis.length === 0 ? (
              <p className={panel.message}>{t("admin:emojisPanel.remoteEmptyMessage")}</p>
            ) : (
              remoteEmojis.map((em) => (
                <div key={em.id} className={styles.row}>
                  <img
                    src={mediaUrl(em.imageUrl)}
                    alt={em.shortcode}
                    style={{ height: 28, width: 28, objectFit: "contain" }}
                  />
                  <div className={styles.grow}>
                    <div className={styles.primaryText}>:{em.shortcode}:</div>
                    <div className={styles.subText}>{em.domain}</div>
                    {em.tags.length > 0 && <div className={styles.subText}>🏷 {em.tags.join(" / ")}</div>}
                    {em.license && <div className={styles.subText}>📄 {em.license}</div>}
                  </div>
                  <button className={styles.btn} onClick={() => setImportTarget(em)}>
                    {t("admin:emojisPanel.remoteImportButton")}
                  </button>
                </div>
              ))
            )}
          </div>
        </>
      )}

      {importTarget && (
        <EmojiImportDialog
          open
          onClose={() => setImportTarget(null)}
          shortcode={importTarget.shortcode}
          imageUrl={importTarget.imageUrl}
          sourceLabel={importTarget.domain}
          initialLicense={importTarget.license}
          onImported={() => {
            if (tab === "local") load();
          }}
        />
      )}
    </div>
  );
}
