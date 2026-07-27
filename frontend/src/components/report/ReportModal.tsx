import { useState } from "react";
import { api, getErrorMessage, ReportReason } from "../../api/client";
import { useToast } from "../../contexts/ToastContext";
import Modal from "../common/Modal";
import styles from "./ReportModal.module.css";

interface Props {
  open: boolean;
  onClose: () => void;
  subjectType: "actor" | "post";
  subjectActorId: string;
  subjectPostId?: string;
  subjectLabel: string;
  remoteHost?: string;
}

const reasons: { value: ReportReason; label: string }[] = [
  { value: "spam", label: "スパム" },
  { value: "violation", label: "規約・法律違反" },
  { value: "misleading", label: "誤解を招く内容" },
  { value: "sexual", label: "性的な内容" },
  { value: "rude", label: "嫌がらせ・攻撃的な内容" },
  { value: "other", label: "その他" },
];

export default function ReportModal(props: Props) {
  const { showError } = useToast();
  const [reason, setReason] = useState<ReportReason>("spam");
  const [text, setText] = useState("");
  const [destination, setDestination] = useState<"local" | "remote">("local");
  const [submitting, setSubmitting] = useState(false);
  const [sent, setSent] = useState(false);

  async function submit() {
    setSubmitting(true);
    try {
      await api.reports.create({
        subject_type: props.subjectType,
        subject_actor_id: props.subjectActorId,
        subject_post_id: props.subjectPostId,
        reason_type: reason,
        reason_text: text,
        destination,
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
    setDestination("local");
    props.onClose();
  }

  return (
    <Modal open={props.open} onClose={close} title="⚠️ 通報">
      {sent ? (
        <>
          <p>通報を受け付けました。</p>
          <button className={styles.primary} onClick={close}>閉じる</button>
        </>
      ) : (
        <div className={styles.form}>
          <p className={styles.subject}>対象: {props.subjectLabel}</p>
          <label>理由
            <select value={reason} onChange={(e) => setReason(e.target.value as ReportReason)}>
              {reasons.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
            </select>
          </label>
          <label>詳細（任意、300文字・1000バイトまで）
            <textarea maxLength={300} value={text} onChange={(e) => setText(e.target.value)} rows={5} />
          </label>
          <fieldset>
            <legend>通報先</legend>
            <label><input type="radio" checked={destination === "local"} onChange={() => setDestination("local")} />
              このサーバーの管理者
            </label>
            {props.remoteHost && (
              <label><input type="radio" checked={destination === "remote"} onChange={() => setDestination("remote")} />
                {props.remoteHost} の管理者・モデレーションサービス
              </label>
            )}
          </fieldset>
          <div className={styles.actions}>
            <button className={styles.primary} onClick={submit} disabled={submitting}>
              {submitting ? "送信中…" : "通報する"}
            </button>
            <button onClick={close}>キャンセル</button>
          </div>
        </div>
      )}
    </Modal>
  );
}
