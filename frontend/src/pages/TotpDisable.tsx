import { useEffect, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type State = { phase: "verifying" } | { phase: "success" } | { phase: "error"; message: string };

/** 認証アプリ・リカバリーコードを両方失った場合のTOTP強制解除リンク（#65）の着地先。 */
export default function TotpDisable() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const [state, setState] = useState<State>({ phase: "verifying" });

  useEffect(() => {
    const token = searchParams.get("token");
    if (!token) {
      setState({ phase: "error", message: t("auth:totpDisable.invalidUrl") });
      return;
    }
    const controller = new AbortController();
    api.auth.totp
      .confirmDisable(token)
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
          <p style={{ textAlign: "center", color: "#a0aec0" }}>{t("auth:totpDisable.verifying")}</p>
        )}
        {state.phase === "success" && (
          <>
            <h2 className={styles.subtitle}>{t("auth:totpDisable.successTitle")}</h2>
            <p style={{ textAlign: "center", color: "#a0aec0" }}>{t("auth:totpDisable.successBody")}</p>
          </>
        )}
        {state.phase === "error" && (
          <>
            <h2 className={styles.subtitle}>{t("auth:totpDisable.failedTitle")}</h2>
            <p className={styles.error} style={{ textAlign: "center" }}>{state.message}</p>
          </>
        )}
        <p className={styles.link} style={{ marginTop: "1rem" }}>
          <Link to="/login">{t("auth:totpVerify.backToLoginLink")}</Link>
        </p>
      </div>
    </div>
  );
}
