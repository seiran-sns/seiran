import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import type { AboutResponse } from "../api/types";
import UserItem from "../components/common/UserItem";
import AppShell from "../components/layout/AppShell";
import { useSiteMeta } from "../contexts/SiteMetaContext";
import styles from "./AboutPage.module.css";

/** http/https 以外（`javascript:` 等）はリンクにしない。サーバーも保存時に弾いているが、
 * 古いデータや直接書き換えた値でも危険なリンクを作らないよう表示側でも確かめる。 */
function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
}

/** 「このサイトについて」（`/about`）。ログイン前後どちらからも開ける。運営者・管理者・
 * 問い合わせ先・利用規約を、設定されているものだけ出す。 */
export default function AboutPage() {
  const { t } = useTranslation();
  const site = useSiteMeta();
  const [about, setAbout] = useState<AboutResponse | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    const controller = new AbortController();
    api
      .about(controller.signal)
      .then(setAbout)
      .catch((e) => {
        if (!controller.signal.aborted) setError(getErrorMessage(e));
      });
    return () => controller.abort();
  }, []);

  const operatorName = about?.operatorName ?? "";
  const operatorUrl = about && isHttpUrl(about.operatorUrl) ? about.operatorUrl : "";

  const center = (
    <div className={styles.page}>
      <div className={styles.header}>
        {site.iconUrl && <img src={site.iconUrl} alt="" className={styles.siteIcon} />}
        <div>
          <p className={styles.pageTitle}>{t("common:aboutPage.title")}</p>
          <h1 className={styles.siteName}>{site.name}</h1>
        </div>
      </div>

      {error && <p className={styles.error}>{error}</p>}

      {(operatorName || operatorUrl) && (
        <section className={styles.section}>
          <h2 className={styles.sectionTitle}>{t("common:aboutPage.operator")}</h2>
          {operatorUrl ? (
            <a href={operatorUrl} target="_blank" rel="noopener noreferrer" className={styles.link}>
              {operatorName || operatorUrl}
            </a>
          ) : (
            <span>{operatorName}</span>
          )}
        </section>
      )}

      {about && about.admins.length > 0 && (
        <section className={styles.section}>
          <h2 className={styles.sectionTitle}>{t("common:aboutPage.admins")}</h2>
          <ul className={styles.adminList}>
            {about.admins.map((admin) => (
              <li key={admin.username}>
                <Link to={`/@${admin.username}`} className={styles.adminLink}>
                  <UserItem
                    avatarUrl={admin.avatarUrl ?? undefined}
                    username={admin.username}
                    displayName={admin.displayName ?? undefined}
                    emojis={admin.emojis}
                    avatarSize={32}
                  />
                  <span className={styles.handle}>@{admin.username}</span>
                </Link>
              </li>
            ))}
          </ul>
        </section>
      )}

      {about?.contactEmail && (
        <section className={styles.section}>
          <h2 className={styles.sectionTitle}>{t("common:aboutPage.contact")}</h2>
          <a href={`mailto:${about.contactEmail}`} className={styles.link}>
            {about.contactEmail}
          </a>
        </section>
      )}

      {site.termsOfServiceHtml.trim() && (
        <section className={styles.section}>
          <h2 className={styles.sectionTitle}>{t("common:aboutPage.termsOfService")}</h2>
          {/* 管理者専用入力のためサニタイズしない（登録画面の利用規約ダイアログと同じ扱い）。 */}
          <div className={styles.terms} dangerouslySetInnerHTML={{ __html: site.termsOfServiceHtml }} />
        </section>
      )}
    </div>
  );

  return <AppShell center={center} />;
}
