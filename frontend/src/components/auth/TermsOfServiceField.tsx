import { useState } from "react";
import { useTranslation } from "react-i18next";
import Modal from "../common/Modal";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import styles from "../../pages/Auth.module.css";

interface TermsOfServiceFieldProps {
  agreed: boolean;
  onChange: (agreed: boolean) => void;
}

/**
 * 新規登録フォーム共通の利用規約同意チェックボックス（サインアップ・Blueskyから転入・
 * メール確認後の登録完了フォームの3箇所から使う）。管理画面の利用規約テキスト
 * （`terms_of_service_text`）が未設定（空文字列）の間は、同意対象が存在しないため
 * チェックボックス自体を表示しない。
 */
export default function TermsOfServiceField({ agreed, onChange }: TermsOfServiceFieldProps) {
  const { t } = useTranslation();
  const { termsOfServiceHtml } = useSiteMeta();
  const [dialogOpen, setDialogOpen] = useState(false);

  if (!termsOfServiceHtml.trim()) return null;

  return (
    <>
      <label className={styles.label} style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
        <input type="checkbox" checked={agreed} onChange={(e) => onChange(e.target.checked)} required />
        {t("auth:termsOfService.checkboxLabel")}
        <button type="button" className={styles.linkButton} onClick={() => setDialogOpen(true)}>
          {t("auth:termsOfService.readButton")}
        </button>
      </label>
      <Modal open={dialogOpen} onClose={() => setDialogOpen(false)} title={t("auth:termsOfService.dialogTitle")}>
        {/* 管理者専用入力のためサニタイズしない（`siteDescriptionHtml`と同じ扱い）。
         * white-space: pre-wrapにより、HTMLタグを使わないプレーンテキスト設定でも
         * 改行がそのまま反映される。 */}
        <div style={{ whiteSpace: "pre-wrap" }} dangerouslySetInnerHTML={{ __html: termsOfServiceHtml }} />
      </Modal>
    </>
  );
}
