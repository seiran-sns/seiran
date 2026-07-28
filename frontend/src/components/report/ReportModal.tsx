import { useMemo, useState } from "react";
import { api, getErrorMessage } from "../../api/client";
import { useToast } from "../../contexts/ToastContext";
import Modal from "../common/Modal";
import { REPORT_CATEGORIES } from "./reportReasons";
import styles from "./ReportModal.module.css";

interface Props {
  open: boolean;
  onClose: () => void;
  subjectType: "actor" | "post";
  subjectActorId: string;
  subjectPostId?: string;
  subjectLabel: string;
}

export default function ReportModal(props: Props) {
  const { showError } = useToast();
  const [categoryKey, setCategoryKey] = useState(REPORT_CATEGORIES[0].key);
  const [reason, setReason] = useState("");
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [sent, setSent] = useState(false);

  const category = useMemo(
    () => REPORT_CATEGORIES.find((c) => c.key === categoryKey) ?? REPORT_CATEGORIES[0],
    [categoryKey],
  );

  function selectCategory(key: string) {
    setCategoryKey(key);
    setReason("");
  }

  async function submit() {
    if (!reason) return;
    setSubmitting(true);
    try {
      await api.reports.create({
        subject_type: props.subjectType,
        subject_actor_id: props.subjectActorId,
        subject_post_id: props.subjectPostId,
        reason_type: reason,
        reason_text: text,
      });
      setSent(true);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setSubmitting(false);
    }
  }

  function close() {
    setSent(false);
    setText("");
    setCategoryKey(REPORT_CATEGORIES[0].key);
    setReason("");
    props.onClose();
  }

  return (
    <Modal open={props.open} onClose={close} title="⚠️ 通報">
      {sent ? (
        <>
          <p>通報を受け付けました。管理者が内容を確認します。</p>
          <button className={styles.primary} onClick={close}>閉じる</button>
        </>
      ) : (
        <div className={styles.form}>
          <p className={styles.subject}>対象: {props.subjectLabel}</p>
          <label>なぜこの{props.subjectType === "post" ? "投稿" : "ユーザー"}をレビューする必要がありますか？
            <select value={categoryKey} onChange={(e) => selectCategory(e.target.value)}>
              {REPORT_CATEGORIES.map((c) => <option key={c.key} value={c.key}>{c.title}</option>)}
            </select>
          </label>
          <p className={styles.categoryDescription}>{category.description}</p>
          <label>理由を選択
            <select value={reason} onChange={(e) => setReason(e.target.value)}>
              <option value="" disabled>選択してください</option>
              {category.options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
            </select>
          </label>
          <label>詳細（任意、300文字・1000バイトまで）
            <textarea maxLength={300} value={text} onChange={(e) => setText(e.target.value)} rows={5} />
          </label>
          <div className={styles.actions}>
            <button className={styles.primary} onClick={submit} disabled={submitting || !reason}>
              {submitting ? "送信中…" : "通報する"}
            </button>
            <button onClick={close}>キャンセル</button>
          </div>
        </div>
      )}
    </Modal>
  );
}
