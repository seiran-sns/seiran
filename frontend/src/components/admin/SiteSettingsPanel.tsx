import { ChangeEvent, FormEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

export default function SiteSettingsPanel() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [passwordSet, setPasswordSet] = useState(false);

  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [from, setFrom] = useState("");
  const [requireVerify, setRequireVerify] = useState(false);

  // サイト外観（#30）
  const [siteName, setSiteName] = useState("");
  const [siteColor, setSiteColor] = useState("");
  const [siteIconUrl, setSiteIconUrl] = useState("");
  const [uploadingIcon, setUploadingIcon] = useState(false);
  const iconRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.admin
      .getSiteSettings()
      .then((s) => {
        setHost(s.smtp_host);
        setPort(s.smtp_port);
        setUsername(s.smtp_username);
        setFrom(s.smtp_from);
        setPasswordSet(s.smtp_password_set);
        setRequireVerify(s.require_email_verification === "true");
        setSiteName(s.site_name);
        setSiteColor(s.site_color);
        setSiteIconUrl(s.site_icon_url);
      })
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, []);

  async function onIcon(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setUploadingIcon(true);
    setError("");
    try {
      const f = await api.media.upload(file, "avatar");
      setSiteIconUrl(f.url);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setUploadingIcon(false);
    }
  }

  async function save(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      const patch: Record<string, string> = {
        smtp_host: host,
        smtp_port: port,
        smtp_username: username,
        smtp_from: from,
        require_email_verification: requireVerify ? "true" : "false",
        site_name: siteName,
        site_color: siteColor,
        site_icon_url: siteIconUrl,
      };
      // パスワードは入力があったときだけ送る（未入力なら既存値を維持）。
      if (password) patch.smtp_password = password;
      const s = await api.admin.updateSiteSettings(patch);
      setPasswordSet(s.smtp_password_set);
      setPassword("");
      setSaved(true);
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:siteSettingsPanel.title")}</h2>
      {error && <p className={styles.error}>{error}</p>}
      {saved && <p className={styles.success}>{t("admin:siteSettingsPanel.savedMessage")}</p>}

      <form onSubmit={save}>
        <div className={styles.card}>
          <div style={{ fontWeight: 700, fontSize: "0.9rem", marginBottom: 12 }}>{t("admin:siteSettingsPanel.appearanceTitle")}</div>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.siteNameLabel")}
            <input className={styles.input} value={siteName} onChange={(e) => setSiteName(e.target.value)} placeholder="seiran" />
          </label>
          <label className={styles.label} style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
            {t("admin:siteSettingsPanel.siteColorLabel")}
            <input type="color" value={siteColor || "#2563eb"} onChange={(e) => setSiteColor(e.target.value)} style={{ width: 48, height: 32, padding: 0, border: "none", background: "none" }} />
            <input className={styles.input} value={siteColor} onChange={(e) => setSiteColor(e.target.value)} placeholder={t("admin:siteSettingsPanel.siteColorPlaceholder")} style={{ flex: 1 }} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.siteIconLabel")}
            <span className={styles.actions} style={{ marginTop: 4 }}>
              <input ref={iconRef} type="file" accept="image/*" style={{ display: "none" }} onChange={onIcon} />
              {siteIconUrl && <img src={siteIconUrl} alt="" style={{ width: 40, height: 40, borderRadius: 8 }} />}
              <button type="button" className={styles.btnGhost} onClick={() => iconRef.current?.click()} disabled={uploadingIcon}>
                {uploadingIcon
                  ? t("admin:siteSettingsPanel.uploading")
                  : siteIconUrl
                    ? t("admin:siteSettingsPanel.changeIconButton")
                    : t("admin:siteSettingsPanel.selectIconButton")}
              </button>
              {siteIconUrl && (
                <button type="button" className={styles.btnGhost} onClick={() => setSiteIconUrl("")}>
                  {t("common:delete")}
                </button>
              )}
            </span>
          </label>
        </div>

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.smtpSectionTitle")}</div>
        <div className={styles.card}>
        <label className={styles.label}>
          {t("admin:siteSettingsPanel.smtpHostLabel")}
          <input className={styles.input} value={host} onChange={(e) => setHost(e.target.value)} placeholder="smtp.resend.com" />
        </label>
        <label className={styles.label}>
          {t("admin:siteSettingsPanel.smtpPortLabel")}
          <input className={styles.input} value={port} onChange={(e) => setPort(e.target.value)} placeholder="587" />
        </label>
        <label className={styles.label}>
          {t("admin:siteSettingsPanel.smtpUsernameLabel")}
          <input className={styles.input} value={username} onChange={(e) => setUsername(e.target.value)} />
        </label>
        <label className={styles.label}>
          {t("admin:siteSettingsPanel.smtpPasswordLabel")}
          <input
            className={styles.input}
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder={passwordSet ? t("admin:siteSettingsPanel.passwordSetPlaceholder") : t("admin:siteSettingsPanel.passwordUnsetPlaceholder")}
          />
        </label>
        <label className={styles.label}>
          {t("admin:siteSettingsPanel.fromAddressLabel")}
          <input className={styles.input} value={from} onChange={(e) => setFrom(e.target.value)} placeholder="info@seiran.org" />
        </label>
        <label className={`${styles.label}`} style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
          <input type="checkbox" checked={requireVerify} onChange={(e) => setRequireVerify(e.target.checked)} />
          {t("admin:siteSettingsPanel.requireVerifyLabel")}
        </label>
        <p className={styles.hint}>
          {t("admin:siteSettingsPanel.requireVerifyHint")}
        </p>
        </div>

        <button className={styles.btn} type="submit" disabled={saving}>
          {saving ? t("common:saving") : t("common:save")}
        </button>
      </form>
    </div>
  );
}
