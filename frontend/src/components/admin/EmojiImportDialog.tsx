import { FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import Modal from "../common/Modal";
import { api, CustomEmoji, getErrorMessage } from "../../api/client";
import { mediaUrl } from "../../utils/mediaProxy";
import styles from "../../pages/Admin.module.css";

interface EmojiImportDialogProps {
  open: boolean;
  onClose: () => void;
  shortcode: string;
  imageUrl: string;
  /** 取り込み元の表示用ラベル（管理画面ではドメイン、NoteCard上では投稿へのリンク等）。 */
  sourceLabel?: string;
  initialLicense?: string | null;
  onImported: (emoji: CustomEmoji) => void;
}

/**
 * リモート絵文字のインポート確認ダイアログ（#73）。
 * 管理画面の「リモート」タブ、NoteCard右クリックメニューの両方から開かれる共通コンポーネント。
 * shortcode は取り込み元のものをそのまま使う（変更不可。issue #73 の要件はカテゴリ・タグ・
 * ライセンスの入力のみ）。
 */
export default function EmojiImportDialog({
  open,
  onClose,
  shortcode,
  imageUrl,
  sourceLabel,
  initialLicense,
  onImported,
}: EmojiImportDialogProps) {
  const { t } = useTranslation();
  const [category, setCategory] = useState("");
  const [tags, setTags] = useState("");
  const [license, setLicense] = useState(initialLicense ?? "");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");

  function reset() {
    setCategory("");
    setTags("");
    setLicense(initialLicense ?? "");
    setError("");
  }

  function handleClose() {
    reset();
    onClose();
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    setError("");
    try {
      const emoji = await api.admin.importRemoteEmoji({
        shortcode,
        image_url: imageUrl,
        category: category.trim() || undefined,
        tags: tags
          .split(/[\s,]+/)
          .map((t) => t.trim())
          .filter(Boolean),
        license: license.trim() || undefined,
      });
      onImported(emoji);
      reset();
      onClose();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal open={open} onClose={handleClose} title={t("admin:emojiImportDialog.title")}>
      <form onSubmit={submit}>
        {error && <p className={styles.error}>{error}</p>}
        <div className={styles.actions} style={{ marginBottom: 12 }}>
          <img src={mediaUrl(imageUrl)} alt={shortcode} style={{ height: 32, width: 32, objectFit: "contain" }} />
          <div>
            <div className={styles.label} style={{ marginBottom: 0 }}>
              {t("admin:emojiImportDialog.shortcodeLabel")}
            </div>
            <div className={styles.primaryText}>:{shortcode}:</div>
          </div>
        </div>
        {sourceLabel && (
          <p className={styles.subText} style={{ marginBottom: 12 }}>
            {t("admin:emojiImportDialog.sourceLabel")}: {sourceLabel}
          </p>
        )}
        <label className={styles.label}>
          {t("admin:emojiImportDialog.categoryLabel")}
          <input className={styles.input} value={category} onChange={(e) => setCategory(e.target.value)} />
        </label>
        <label className={styles.label}>
          {t("admin:emojiImportDialog.tagsLabel")}
          <input
            className={styles.input}
            value={tags}
            onChange={(e) => setTags(e.target.value)}
            placeholder={t("admin:emojiImportDialog.tagsPlaceholder")}
          />
        </label>
        <label className={styles.label}>
          {t("admin:emojiImportDialog.licenseLabel")}
          <input
            className={styles.input}
            value={license}
            onChange={(e) => setLicense(e.target.value)}
            placeholder={t("admin:emojiImportDialog.licensePlaceholder")}
          />
        </label>
        <div className={styles.actions}>
          <button className={styles.btn} type="submit" disabled={submitting}>
            {submitting ? t("admin:emojiImportDialog.submitting") : t("admin:emojiImportDialog.submitButton")}
          </button>
          <button className={styles.btnGhost} type="button" onClick={handleClose} disabled={submitting}>
            {t("admin:emojiImportDialog.cancelButton")}
          </button>
        </div>
      </form>
    </Modal>
  );
}
