import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation();
  const { showError } = useToast();
  const [categoryKey, setCategoryKey] = useState(REPORT_CATEGORIES[0].key);
  const [reason, setReason] = useState("");
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [sent, setSent] = useState(false);

  const category = useMemo(
    () =>
      REPORT_CATEGORIES.find((c) => c.key === categoryKey) ??
      REPORT_CATEGORIES[0],
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
    <Modal
      open={props.open}
      onClose={close}
      title={t("admin:reports.modalTitle")}
    >
      {sent ? (
        <>
          <p>{t("admin:reports.sent")}</p>
          <button className={styles.primary} onClick={close}>
            {t("common:close")}
          </button>
        </>
      ) : (
        <div className={styles.form}>
          <p className={styles.subject}>
            {t("admin:reports.subject", { subject: props.subjectLabel })}
          </p>
          <label>
            {t(
              `admin:reports.${props.subjectType === "post" ? "questionPost" : "questionActor"}`,
            )}
            <select
              value={categoryKey}
              onChange={(e) => selectCategory(e.target.value)}
            >
              {REPORT_CATEGORIES.map((c) => (
                <option key={c.key} value={c.key}>
                  {t(`admin:reports.categories.${c.key}.title`)}
                </option>
              ))}
            </select>
          </label>
          <p className={styles.categoryDescription}>
            {t(`admin:reports.categories.${category.key}.description`)}
          </p>
          <label>
            {t("admin:reports.reasonLabel")}
            <select value={reason} onChange={(e) => setReason(e.target.value)}>
              <option value="" disabled>
                {t("admin:reports.selectPlaceholder")}
              </option>
              {category.options.map((value) => (
                <option key={value} value={value}>
                  {t(`admin:reports.reasons.${value}`)}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("admin:reports.detailsLabel")}
            <textarea
              maxLength={300}
              value={text}
              onChange={(e) => setText(e.target.value)}
              rows={5}
            />
          </label>
          <div className={styles.actions}>
            <button
              className={styles.primary}
              onClick={submit}
              disabled={submitting || !reason}
            >
              {submitting
                ? t("admin:reports.submitting")
                : t("admin:reports.submit")}
            </button>
            <button onClick={close}>{t("common:cancel")}</button>
          </div>
        </div>
      )}
    </Modal>
  );
}
