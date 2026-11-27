import { ChangeEvent, FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { api, getErrorMessage } from "../../api/client";
import type { SmtpTestResult, TurnstileTestResult } from "../../api/admin";
import type { SiteSettings } from "../../api/types";
import { diffEdits } from "../../lib/editDiff";
import Turnstile from "../Turnstile";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

/** この画面が編集する項目（書き込み専用の項目と `*_set` を除く）。 */
const FORM_KEYS = [
  "smtp_host", "smtp_port", "smtp_username", "smtp_from", "require_email_verification",
  "site_name", "site_color", "site_icon_url", "site_icon_sha256", "site_icon_media_file_id",
  "media_proxy_url", "site_description", "terms_of_service_text", "operator_name",
  "operator_url", "contact_email", "login_bg_url", "login_bg_type", "login_bg_media_file_id",
  "auth_bruteforce_window_minutes", "auth_bruteforce_max_variants",
  "auth_ip_block_window_minutes", "auth_ip_block_threshold", "auth_ip_block_duration_hours",
  "turnstile_site_key", "cloudflare_zone_id", "password_reset_max_active",
  "account_creation_ip_window_minutes", "account_creation_ip_max", "oembed_allowed_domains",
  "max_user_accounts", "registration_closed",
] as const satisfies readonly (keyof SiteSettings)[];
type FormValues = Record<(typeof FORM_KEYS)[number], string>;

function formFromSettings(s: SiteSettings | null): FormValues {
  return Object.fromEntries(FORM_KEYS.map((k) => [k, s?.[k] ?? ""])) as FormValues;
}

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
  const [testRecipient, setTestRecipient] = useState("");
  const [smtpTesting, setSmtpTesting] = useState<"" | "connection" | "email">("");
  const [smtpTestMessage, setSmtpTestMessage] = useState<{ ok: boolean; text: string } | null>(null);

  // サイト外観（#30）
  const [siteName, setSiteName] = useState("");
  const [siteColor, setSiteColor] = useState("");
  const [siteIconUrl, setSiteIconUrl] = useState("");
  const [siteIconSha256, setSiteIconSha256] = useState("");
  const [siteIconMediaFileId, setSiteIconMediaFileId] = useState("");
  const [mediaProxyUrl, setMediaProxyUrl] = useState("");
  const [uploadingIcon, setUploadingIcon] = useState(false);
  const [iconError, setIconError] = useState("");
  const iconRef = useRef<HTMLInputElement>(null);

  // ログイン画面デザイン（#243）
  const [siteDescription, setSiteDescription] = useState("");
  const [termsOfServiceText, setTermsOfServiceText] = useState("");
  // 「このサイトについて」（/about）
  const [operatorName, setOperatorName] = useState("");
  const [operatorUrl, setOperatorUrl] = useState("");
  const [contactEmail, setContactEmail] = useState("");
  const [loginBgUrl, setLoginBgUrl] = useState("");
  const [loginBgType, setLoginBgType] = useState("");
  const [loginBgMediaFileId, setLoginBgMediaFileId] = useState("");
  const [uploadingBg, setUploadingBg] = useState(false);
  const [bgError, setBgError] = useState("");
  const bgRef = useRef<HTMLInputElement>(null);

  // 認証系レート制限（#223）
  const [bruteforceWindowMinutes, setBruteforceWindowMinutes] = useState("");
  const [bruteforceMaxVariants, setBruteforceMaxVariants] = useState("");
  const [ipBlockWindowMinutes, setIpBlockWindowMinutes] = useState("");
  const [ipBlockThreshold, setIpBlockThreshold] = useState("");
  const [ipBlockDurationHours, setIpBlockDurationHours] = useState("");
  const [turnstileSiteKey, setTurnstileSiteKey] = useState("");
  const [turnstileSecretKey, setTurnstileSecretKey] = useState("");
  const [turnstileSecretKeySet, setTurnstileSecretKeySet] = useState(false);
  // 動作確認: "widget" の間はウィジェットを出してトークン待ち、"verifying" は Siteverify 待ち
  const [turnstileTesting, setTurnstileTesting] = useState<"" | "widget" | "verifying">("");
  const [turnstileTestMessage, setTurnstileTestMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const [cloudflareApiToken, setCloudflareApiToken] = useState("");
  const [cloudflareApiTokenSet, setCloudflareApiTokenSet] = useState(false);
  const [cloudflareZoneId, setCloudflareZoneId] = useState("");
  const [passwordResetMaxActive, setPasswordResetMaxActive] = useState("");
  const [accountCreationIpWindowMinutes, setAccountCreationIpWindowMinutes] = useState("");
  const [accountCreationIpMax, setAccountCreationIpMax] = useState("");
  const [maxUserAccounts, setMaxUserAccounts] = useState("");
  const [registrationClosed, setRegistrationClosed] = useState(false);

  // 有効なストレージプロバイダーが無いとアップロードは必ず失敗する（503
  // STORAGE_NOT_CONFIGURED）ので、アイコン・背景の選択ボタンを押せなくして理由を示す。
  // 取得できなかったときは押せるままにし、失敗はアップロード時のエラー表示に任せる。
  const [storageConfigured, setStorageConfigured] = useState(true);

  // URLカード埋め込みプレーヤー（oEmbed discovery）の許可ドメイン
  const [oembedAllowedDomains, setOembedAllowedDomains] = useState("");

  /** 開いた（最後に保存した）時点のサーバーの値。保存はここからの差分で送る。 */
  const baseline = useRef<FormValues>(formFromSettings(null));

  /** 画面の値（書き込み専用の項目を除く）。キーはサイト設定の項目名。 */
  function formValues(): FormValues {
    return {
      smtp_host: host,
      smtp_port: port,
      smtp_username: username,
      smtp_from: from,
      require_email_verification: requireVerify ? "true" : "false",
      site_name: siteName,
      site_color: siteColor,
      site_icon_url: siteIconUrl,
      site_icon_sha256: siteIconSha256,
      site_icon_media_file_id: siteIconMediaFileId,
      media_proxy_url: mediaProxyUrl,
      site_description: siteDescription,
      terms_of_service_text: termsOfServiceText,
      operator_name: operatorName,
      operator_url: operatorUrl,
      contact_email: contactEmail,
      login_bg_url: loginBgUrl,
      login_bg_type: loginBgType,
      login_bg_media_file_id: loginBgMediaFileId,
      auth_bruteforce_window_minutes: bruteforceWindowMinutes,
      auth_bruteforce_max_variants: bruteforceMaxVariants,
      auth_ip_block_window_minutes: ipBlockWindowMinutes,
      auth_ip_block_threshold: ipBlockThreshold,
      auth_ip_block_duration_hours: ipBlockDurationHours,
      turnstile_site_key: turnstileSiteKey,
      cloudflare_zone_id: cloudflareZoneId,
      password_reset_max_active: passwordResetMaxActive,
      account_creation_ip_window_minutes: accountCreationIpWindowMinutes,
      account_creation_ip_max: accountCreationIpMax,
      oembed_allowed_domains: oembedAllowedDomains,
      max_user_accounts: maxUserAccounts,
      registration_closed: registrationClosed ? "true" : "false",
    };
  }

  /** サーバーの値を画面に出し、差分の基準にする。 */
  function applySettings(s: SiteSettings) {
    const f = formFromSettings(s);
    baseline.current = f;
    setHost(f.smtp_host);
    setPort(f.smtp_port);
    setUsername(f.smtp_username);
    setFrom(f.smtp_from);
    setRequireVerify(f.require_email_verification === "true");
    setSiteName(f.site_name);
    setSiteColor(f.site_color);
    setSiteIconUrl(f.site_icon_url);
    setSiteIconSha256(f.site_icon_sha256);
    setSiteIconMediaFileId(f.site_icon_media_file_id);
    setMediaProxyUrl(f.media_proxy_url);
    setSiteDescription(f.site_description);
    setTermsOfServiceText(f.terms_of_service_text);
    setOperatorName(f.operator_name);
    setOperatorUrl(f.operator_url);
    setContactEmail(f.contact_email);
    setLoginBgUrl(f.login_bg_url);
    setLoginBgType(f.login_bg_type);
    setLoginBgMediaFileId(f.login_bg_media_file_id);
    setBruteforceWindowMinutes(f.auth_bruteforce_window_minutes);
    setBruteforceMaxVariants(f.auth_bruteforce_max_variants);
    setIpBlockWindowMinutes(f.auth_ip_block_window_minutes);
    setIpBlockThreshold(f.auth_ip_block_threshold);
    setIpBlockDurationHours(f.auth_ip_block_duration_hours);
    setTurnstileSiteKey(f.turnstile_site_key);
    setCloudflareZoneId(f.cloudflare_zone_id);
    setPasswordResetMaxActive(f.password_reset_max_active);
    setAccountCreationIpWindowMinutes(f.account_creation_ip_window_minutes);
    setAccountCreationIpMax(f.account_creation_ip_max);
    setOembedAllowedDomains(f.oembed_allowed_domains);
    setMaxUserAccounts(f.max_user_accounts);
    setRegistrationClosed(f.registration_closed === "true");
    setPasswordSet(s.smtp_password_set);
    setTurnstileSecretKeySet(s.turnstile_secret_key_set);
    setCloudflareApiTokenSet(s.cloudflare_api_token_set);
  }

  useEffect(() => {
    api.admin
      .getSiteSettings()
      .then(applySettings)
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
    api.admin
      .listStorageProviders()
      .then((providers) => setStorageConfigured(providers.some((p) => p.is_active)))
      .catch(() => {});
  }, []);

  async function onIcon(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setUploadingIcon(true);
    setIconError("");
    try {
      const f = await api.media.upload(file, "avatar");
      setSiteIconUrl(f.url);
      setSiteIconSha256(f.sha256);
      setSiteIconMediaFileId(f.id);
    } catch (err) {
      setIconError(getErrorMessage(err));
    } finally {
      setUploadingIcon(false);
    }
  }

  async function onBg(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setUploadingBg(true);
    setBgError("");
    try {
      // ログイン画面背景はどの投稿にも属さないので、Bsky動画配信パイプラインへは提出しない。
      const f = await api.media.upload(file, "login_background", false);
      setLoginBgUrl(f.url);
      setLoginBgType(f.mimeType.startsWith("video/") ? "video" : "image");
      setLoginBgMediaFileId(f.id);
    } catch (err) {
      setBgError(getErrorMessage(err));
    } finally {
      setUploadingBg(false);
    }
  }

  function smtpTestMessageText(kind: "connection" | "email", r: SmtpTestResult): string {
    if (r.ok) {
      return kind === "connection"
        ? t("admin:siteSettingsPanel.smtpConnectionOk")
        : t("admin:siteSettingsPanel.smtpTestEmailSent");
    }
    const reason = t(`admin:siteSettingsPanel.smtpTestFailure.${r.code ?? "SMTP_FAILED"}`);
    return r.detail ? `${reason}: ${r.detail}` : reason;
  }

  // 保存前のフォームの値で試す。パスワード欄が空なら保存済みのパスワードを使う。
  async function runSmtpTest(kind: "connection" | "email") {
    setSmtpTesting(kind);
    setSmtpTestMessage(null);
    const body = {
      smtp_host: host,
      smtp_port: port,
      smtp_username: username,
      smtp_from: from,
      ...(password ? { smtp_password: password } : {}),
    };
    try {
      const r =
        kind === "connection"
          ? await api.admin.testSmtpConnection(body)
          : await api.admin.sendSmtpTestEmail({ ...body, to: testRecipient });
      setSmtpTestMessage({ ok: r.ok, text: smtpTestMessageText(kind, r) });
    } catch (err) {
      setSmtpTestMessage({ ok: false, text: getErrorMessage(err) });
    } finally {
      setSmtpTesting("");
    }
  }

  // 保存前のフォームの値で試す。Site Key でウィジェットを出し、得たトークンを入力中の
  // Secret Key（空なら保存済みの値）で Siteverify にかける。
  const onTurnstileTestToken = useCallback(
    async (token: string) => {
      if (!token) return;
      setTurnstileTesting("verifying");
      try {
        const r = await api.admin.testTurnstile({
          token,
          ...(turnstileSecretKey ? { turnstile_secret_key: turnstileSecretKey } : {}),
        });
        setTurnstileTestMessage({ ok: r.ok, text: turnstileTestMessageText(t, r) });
      } catch (err) {
        setTurnstileTestMessage({ ok: false, text: getErrorMessage(err) });
      } finally {
        setTurnstileTesting("");
      }
    },
    [turnstileSecretKey, t],
  );

  const onTurnstileTestError = useCallback(
    (code: string) => {
      setTurnstileTesting("");
      setTurnstileTestMessage({
        ok: false,
        text: `${t("admin:siteSettingsPanel.turnstileTestFailure.WIDGET")}: ${code}`,
      });
    },
    [t],
  );

  async function save(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      // 開いた（最後に保存した）時点から変えた項目だけを差分で送る
      const patch: Parameters<typeof api.admin.updateSiteSettings>[0] = diffEdits(
        baseline.current,
        formValues(),
      );
      // パスワード/シークレットは入力があったときだけ送る（未入力なら既存値を維持）。
      if (password) patch.smtp_password = password;
      if (turnstileSecretKey) patch.turnstile_secret_key = turnstileSecretKey;
      if (cloudflareApiToken) patch.cloudflare_api_token = cloudflareApiToken;
      // 別の画面の変更も取り込んだ保存後の値を表示し直す
      applySettings(await api.admin.updateSiteSettings(patch));
      setPassword("");
      setTurnstileSecretKey("");
      setCloudflareApiToken("");
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

      <form onSubmit={save}>
        <div className={styles.card}>
          <div style={{ fontWeight: 700, fontSize: "0.9rem", marginBottom: 12 }}>{t("admin:siteSettingsPanel.appearanceTitle")}</div>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.siteNameLabel")}
            <input className={styles.input} value={siteName} onChange={(e) => setSiteName(e.target.value)} placeholder="seiran" />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.siteNameHtmlHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.siteDescriptionLabel")}
            <textarea
              className={styles.input}
              rows={4}
              value={siteDescription}
              onChange={(e) => setSiteDescription(e.target.value)}
            />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.siteDescriptionHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.termsOfServiceLabel")}
            <textarea
              className={styles.input}
              rows={8}
              value={termsOfServiceText}
              onChange={(e) => setTermsOfServiceText(e.target.value)}
            />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.termsOfServiceHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.operatorNameLabel")}
            <input className={styles.input} value={operatorName} onChange={(e) => setOperatorName(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.operatorUrlLabel")}
            <input
              className={styles.input}
              type="url"
              value={operatorUrl}
              onChange={(e) => setOperatorUrl(e.target.value)}
              placeholder="https://example.com/"
            />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.contactEmailLabel")}
            <input
              className={styles.input}
              type="email"
              value={contactEmail}
              onChange={(e) => setContactEmail(e.target.value)}
            />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.aboutFieldsHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.loginBackgroundLabel")}
            <span className={styles.actions} style={{ marginTop: 4 }}>
              <input ref={bgRef} type="file" accept="image/*,video/*" style={{ display: "none" }} onChange={onBg} />
              {loginBgUrl && loginBgType === "video" && (
                <video src={loginBgUrl} muted loop style={{ width: 80, height: 45, borderRadius: 8, objectFit: "cover" }} />
              )}
              {loginBgUrl && loginBgType !== "video" && (
                <img src={loginBgUrl} alt="" style={{ width: 80, height: 45, borderRadius: 8, objectFit: "cover" }} />
              )}
              <button
                type="button"
                className={styles.btnGhost}
                onClick={() => bgRef.current?.click()}
                disabled={uploadingBg || !storageConfigured}
                title={storageConfigured ? undefined : t("admin:siteSettingsPanel.storageNotConfigured")}
              >
                {uploadingBg
                  ? t("admin:siteSettingsPanel.uploading")
                  : loginBgUrl
                    ? t("admin:siteSettingsPanel.changeBackgroundButton")
                    : t("admin:siteSettingsPanel.selectBackgroundButton")}
              </button>
              {loginBgUrl && (
                <button
                  type="button"
                  className={styles.btnGhost}
                  onClick={() => {
                    setLoginBgUrl("");
                    setLoginBgType("");
                    setLoginBgMediaFileId("");
                  }}
                >
                  {t("common:delete")}
                </button>
              )}
            </span>
          </label>
          {bgError && <p className={styles.error}>{bgError}</p>}
          <p className={styles.hint}>{t("admin:siteSettingsPanel.loginBackgroundHint")}</p>
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
              <button
                type="button"
                className={styles.btnGhost}
                onClick={() => iconRef.current?.click()}
                disabled={uploadingIcon || !storageConfigured}
                title={storageConfigured ? undefined : t("admin:siteSettingsPanel.storageNotConfigured")}
              >
                {uploadingIcon
                  ? t("admin:siteSettingsPanel.uploading")
                  : siteIconUrl
                    ? t("admin:siteSettingsPanel.changeIconButton")
                    : t("admin:siteSettingsPanel.selectIconButton")}
              </button>
              {siteIconUrl && (
                <button
                  type="button"
                  className={styles.btnGhost}
                  onClick={() => {
                    setSiteIconUrl("");
                    setSiteIconSha256("");
                    setSiteIconMediaFileId("");
                  }}
                >
                  {t("common:delete")}
                </button>
              )}
            </span>
          </label>
          {iconError && <p className={styles.error}>{iconError}</p>}
        </div>

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.mediaProxyTitle")}</div>
        <div className={styles.card}>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.mediaProxyUrlLabel")}
            <input className={styles.input} value={mediaProxyUrl} onChange={(e) => setMediaProxyUrl(e.target.value)} placeholder="https://media-proxy.example/proxy" />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.mediaProxyHint")}</p>
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
        <div className={styles.actions} style={{ marginBottom: 8 }}>
          <button
            type="button"
            className={styles.btnGhost}
            onClick={() => runSmtpTest("connection")}
            disabled={smtpTesting !== ""}
          >
            {smtpTesting === "connection"
              ? t("admin:siteSettingsPanel.smtpTesting")
              : t("admin:siteSettingsPanel.smtpTestConnectionButton")}
          </button>
        </div>
        <label className={styles.label}>
          {t("admin:siteSettingsPanel.smtpTestRecipientLabel")}
          <input
            className={styles.input}
            type="email"
            value={testRecipient}
            onChange={(e) => setTestRecipient(e.target.value)}
            placeholder="admin@example.com"
          />
        </label>
        <div className={styles.actions}>
          <button
            type="button"
            className={styles.btnGhost}
            onClick={() => runSmtpTest("email")}
            disabled={smtpTesting !== "" || testRecipient.trim() === ""}
          >
            {smtpTesting === "email"
              ? t("admin:siteSettingsPanel.smtpTesting")
              : t("admin:siteSettingsPanel.smtpSendTestEmailButton")}
          </button>
        </div>
        {smtpTestMessage && (
          <p className={smtpTestMessage.ok ? styles.success : styles.error} role="status">
            {smtpTestMessage.text}
          </p>
        )}
        <p className={styles.hint}>{t("admin:siteSettingsPanel.smtpTestHint")}</p>
        <label className={`${styles.label}`} style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
          <input type="checkbox" checked={requireVerify} onChange={(e) => setRequireVerify(e.target.checked)} />
          {t("admin:siteSettingsPanel.requireVerifyLabel")}
        </label>
        <p className={styles.hint}>
          {t("admin:siteSettingsPanel.requireVerifyHint")}
        </p>
        </div>

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.authRateLimitSectionTitle")}</div>
        <div className={styles.card}>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.bruteforceWindowMinutesLabel")}
            <input className={styles.input} type="number" min={1} value={bruteforceWindowMinutes} onChange={(e) => setBruteforceWindowMinutes(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.bruteforceMaxVariantsLabel")}
            <input className={styles.input} type="number" min={1} value={bruteforceMaxVariants} onChange={(e) => setBruteforceMaxVariants(e.target.value)} />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.bruteforceHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.ipBlockWindowMinutesLabel")}
            <input className={styles.input} type="number" min={1} value={ipBlockWindowMinutes} onChange={(e) => setIpBlockWindowMinutes(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.ipBlockThresholdLabel")}
            <input className={styles.input} type="number" min={1} value={ipBlockThreshold} onChange={(e) => setIpBlockThreshold(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.ipBlockDurationHoursLabel")}
            <input className={styles.input} type="number" min={1} value={ipBlockDurationHours} onChange={(e) => setIpBlockDurationHours(e.target.value)} />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.ipBlockHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.passwordResetMaxActiveLabel")}
            <input className={styles.input} type="number" min={1} value={passwordResetMaxActive} onChange={(e) => setPasswordResetMaxActive(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.accountCreationIpWindowMinutesLabel")}
            <input className={styles.input} type="number" min={1} value={accountCreationIpWindowMinutes} onChange={(e) => setAccountCreationIpWindowMinutes(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.accountCreationIpMaxLabel")}
            <input className={styles.input} type="number" min={1} value={accountCreationIpMax} onChange={(e) => setAccountCreationIpMax(e.target.value)} />
          </label>
          <label className={styles.label} style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
            <input type="checkbox" checked={registrationClosed} onChange={(e) => setRegistrationClosed(e.target.checked)} />
            {t("admin:siteSettingsPanel.registrationClosedLabel")}
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.registrationClosedHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.maxUserAccountsLabel")}
            <input className={styles.input} type="number" min={0} value={maxUserAccounts} onChange={(e) => setMaxUserAccounts(e.target.value)} />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.maxUserAccountsHint")}</p>
        </div>

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.turnstileSectionTitle")}</div>
        <div className={styles.card}>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.turnstileSiteKeyLabel")}
            <input className={styles.input} value={turnstileSiteKey} onChange={(e) => setTurnstileSiteKey(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.turnstileSecretKeyLabel")}
            <input
              className={styles.input}
              type="password"
              value={turnstileSecretKey}
              onChange={(e) => setTurnstileSecretKey(e.target.value)}
              placeholder={turnstileSecretKeySet ? t("admin:siteSettingsPanel.passwordSetPlaceholder") : t("admin:siteSettingsPanel.passwordUnsetPlaceholder")}
            />
          </label>
          <div className={styles.actions} style={{ marginBottom: 8 }}>
            <button
              type="button"
              className={styles.btnGhost}
              onClick={() => {
                setTurnstileTestMessage(null);
                setTurnstileTesting("widget");
              }}
              disabled={
                turnstileTesting !== "" ||
                turnstileSiteKey.trim() === "" ||
                (turnstileSecretKey === "" && !turnstileSecretKeySet)
              }
            >
              {turnstileTesting === ""
                ? t("admin:siteSettingsPanel.turnstileTestButton")
                : t("admin:siteSettingsPanel.smtpTesting")}
            </button>
          </div>
          {turnstileTesting === "widget" && (
            <>
              <p className={styles.hint}>{t("admin:siteSettingsPanel.turnstileTestSolve")}</p>
              <Turnstile
                siteKey={turnstileSiteKey.trim()}
                onToken={onTurnstileTestToken}
                onError={onTurnstileTestError}
              />
            </>
          )}
          {turnstileTestMessage && (
            <p className={turnstileTestMessage.ok ? styles.success : styles.error} role="status">
              {turnstileTestMessage.text}
            </p>
          )}
          <p className={styles.hint}>{t("admin:siteSettingsPanel.turnstileTestHint")}</p>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.turnstileHint")}</p>
        </div>

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.cloudflareDnsSectionTitle")}</div>
        <div className={styles.card}>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.cloudflareApiTokenLabel")}
            <input
              className={styles.input}
              type="password"
              value={cloudflareApiToken}
              onChange={(e) => setCloudflareApiToken(e.target.value)}
              placeholder={cloudflareApiTokenSet ? t("admin:siteSettingsPanel.passwordSetPlaceholder") : t("admin:siteSettingsPanel.passwordUnsetPlaceholder")}
            />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.cloudflareZoneIdLabel")}
            <input className={styles.input} value={cloudflareZoneId} onChange={(e) => setCloudflareZoneId(e.target.value)} />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.cloudflareDnsHint")}</p>
        </div>

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.oembedSectionTitle")}</div>
        <div className={styles.card}>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.oembedAllowedDomainsLabel")}
            <textarea
              className={styles.input}
              rows={6}
              value={oembedAllowedDomains}
              onChange={(e) => setOembedAllowedDomains(e.target.value)}
              placeholder={
                "youtube.com\nopen.spotify.com\nmusic.apple.com\nsoundcloud.com\nvimeo.com,https://vimeo.com/api/oembed.json"
              }
            />
          </label>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.oembedAllowedDomainsHint")}</p>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.oembedEndpointHint")}</p>
        </div>

        <div className={styles.stickySaveBar}>
          {/* 保存結果は押した場所の近くに出す（ボタンが下端固定のため、先頭に出すと見えない）。 */}
          {error && <p className={styles.error}>{error}</p>}
          {saved && <p className={styles.success}>{t("admin:siteSettingsPanel.savedMessage")}</p>}
          <button className={styles.btn} type="submit" disabled={saving}>
            {saving ? t("common:saving") : t("common:save")}
          </button>
        </div>
      </form>
    </div>
  );
}

function turnstileTestMessageText(t: TFunction, r: TurnstileTestResult): string {
  if (r.ok) return t("admin:siteSettingsPanel.turnstileTestOk", { hostname: r.hostname ?? "-" });
  const reason = t(`admin:siteSettingsPanel.turnstileTestFailure.${r.code ?? "TURNSTILE_FAILED"}`);
  return r.detail ? `${reason}: ${r.detail}` : reason;
}
