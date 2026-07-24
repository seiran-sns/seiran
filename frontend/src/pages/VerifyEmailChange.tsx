import { useEffect, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type State = { phase: "verifying" } | { phase: "success" } | { phase: "error"; message: string };

/** 設定画面のメールアドレス変更（#59）確認リンク（`/verify-email-change?token=...`）の着地先。 */
export default function VerifyEmailChange() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const [state, setState] = useState<State>({ phase: "verifying" });

  useEffect(() => {
    const token = searchParams.get("token");
    if (!token) {
      setState({ phase: "error", message: t("account:verifyEmailChange.invalidUrl") });
      return;
    }
    const controller = new AbortController();
    api.account
      .confirmEmailChange(token)
      .then(() => !controller.signal.aborted && setState({ phase: "success" }))
      .catch((err) => {
        if (controller.signal.aborted) return;
        setState({ phase: "error", message: getErrorMessage(err) });
      });
    return () => controller.abort();
  }, [searchParams, t]);

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        {state.phase === "verifying" && (
          <p style={{ textAlign: "center", color: "#a0aec0" }}>{t("account:verifyEmailChange.verifying")}</p>
        )}
        {state.phase === "success" && (
          <>
            <h2 className={styles.subtitle}>{t("account:verifyEmailChange.successTitle")}</h2>
            <p style={{ textAlign: "center", color: "#a0aec0" }}>{t("account:verifyEmailChange.successBody")}</p>
          </>
        )}
        {state.phase === "error" && (
          <>
            <h2 className={styles.subtitle}>{t("account:verifyEmailChange.failedTitle")}</h2>
            <p className={styles.error} style={{ textAlign: "center" }}>{state.message}</p>
          </>
        )}
        <p className={styles.link} style={{ marginTop: "1rem" }}>
          <Link to="/settings/account">{t("account:verifyEmailChange.backToSettingsLink")}</Link>
        </p>
      </div>
    </div>
  );
}
