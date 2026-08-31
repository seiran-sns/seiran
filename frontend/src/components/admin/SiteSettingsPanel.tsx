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
  const [siteIconSha256, setSiteIconSha256] = useState("");
  const [mediaProxyUrl, setMediaProxyUrl] = useState("");
  const [uploadingIcon, setUploadingIcon] = useState(false);
  const iconRef = useRef<HTMLInputElement>(null);

  // 認証系レート制限（#223）
  const [bruteforceWindowMinutes, setBruteforceWindowMinutes] = useState("");
  const [bruteforceMaxVariants, setBruteforceMaxVariants] = useState("");
  const [ipBlockWindowMinutes, setIpBlockWindowMinutes] = useState("");
  const [ipBlockThreshold, setIpBlockThreshold] = useState("");
  const [ipBlockDurationHours, setIpBlockDurationHours] = useState("");
  const [turnstileSiteKey, setTurnstileSiteKey] = useState("");
  const [turnstileSecretKey, setTurnstileSecretKey] = useState("");
  const [turnstileSecretKeySet, setTurnstileSecretKeySet] = useState(false);
  const [passwordResetMaxActive, setPasswordResetMaxActive] = useState("");
  const [accountCreationIpWindowMinutes, setAccountCreationIpWindowMinutes] = useState("");
  const [accountCreationIpMax, setAccountCreationIpMax] = useState("");

  // ロール別レート制限（#223フォローアップ）
  const [postRateLimitWindowMinutes, setPostRateLimitWindowMinutes] = useState("");
  const [postRateLimitMaxUser, setPostRateLimitMaxUser] = useState("");
  const [postRateLimitMaxModerator, setPostRateLimitMaxModerator] = useState("");
  const [followRateLimitWindowHours, setFollowRateLimitWindowHours] = useState("");
  const [followRateLimitMaxUser, setFollowRateLimitMaxUser] = useState("");
  const [followRateLimitMaxModerator, setFollowRateLimitMaxModerator] = useState("");
  const [listMaxCountUser, setListMaxCountUser] = useState("");
  const [listMaxCountModerator, setListMaxCountModerator] = useState("");
  const [listMemberMaxUser, setListMemberMaxUser] = useState("");
  const [listMemberMaxModerator, setListMemberMaxModerator] = useState("");
  const [searchRateLimitWindowMinutes, setSearchRateLimitWindowMinutes] = useState("");
  const [searchRateLimitMaxUser, setSearchRateLimitMaxUser] = useState("");
  const [searchRateLimitMaxModerator, setSearchRateLimitMaxModerator] = useState("");

  // URLカード埋め込みプレーヤー（oEmbed discovery）の許可ドメイン
  const [oembedAllowedDomains, setOembedAllowedDomains] = useState("");

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
        setSiteIconSha256(s.site_icon_sha256);
        setMediaProxyUrl(s.media_proxy_url);
        setBruteforceWindowMinutes(s.auth_bruteforce_window_minutes);
        setBruteforceMaxVariants(s.auth_bruteforce_max_variants);
        setIpBlockWindowMinutes(s.auth_ip_block_window_minutes);
        setIpBlockThreshold(s.auth_ip_block_threshold);
        setIpBlockDurationHours(s.auth_ip_block_duration_hours);
        setTurnstileSiteKey(s.turnstile_site_key);
        setTurnstileSecretKeySet(s.turnstile_secret_key_set);
        setPasswordResetMaxActive(s.password_reset_max_active);
        setAccountCreationIpWindowMinutes(s.account_creation_ip_window_minutes);
        setAccountCreationIpMax(s.account_creation_ip_max);
        setPostRateLimitWindowMinutes(s.post_rate_limit_window_minutes);
        setPostRateLimitMaxUser(s.post_rate_limit_max_user);
        setPostRateLimitMaxModerator(s.post_rate_limit_max_moderator);
        setFollowRateLimitWindowHours(s.follow_rate_limit_window_hours);
        setFollowRateLimitMaxUser(s.follow_rate_limit_max_user);
        setFollowRateLimitMaxModerator(s.follow_rate_limit_max_moderator);
        setListMaxCountUser(s.list_max_count_user);
        setListMaxCountModerator(s.list_max_count_moderator);
        setListMemberMaxUser(s.list_member_max_user);
        setListMemberMaxModerator(s.list_member_max_moderator);
        setSearchRateLimitWindowMinutes(s.search_rate_limit_window_minutes);
        setSearchRateLimitMaxUser(s.search_rate_limit_max_user);
        setSearchRateLimitMaxModerator(s.search_rate_limit_max_moderator);
        setOembedAllowedDomains(s.oembed_allowed_domains);
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
      setSiteIconSha256(f.sha256);
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
        site_icon_sha256: siteIconSha256,
        media_proxy_url: mediaProxyUrl,
        auth_bruteforce_window_minutes: bruteforceWindowMinutes,
        auth_bruteforce_max_variants: bruteforceMaxVariants,
        auth_ip_block_window_minutes: ipBlockWindowMinutes,
        auth_ip_block_threshold: ipBlockThreshold,
        auth_ip_block_duration_hours: ipBlockDurationHours,
        turnstile_site_key: turnstileSiteKey,
        password_reset_max_active: passwordResetMaxActive,
        account_creation_ip_window_minutes: accountCreationIpWindowMinutes,
        account_creation_ip_max: accountCreationIpMax,
        post_rate_limit_window_minutes: postRateLimitWindowMinutes,
        post_rate_limit_max_user: postRateLimitMaxUser,
        post_rate_limit_max_moderator: postRateLimitMaxModerator,
        follow_rate_limit_window_hours: followRateLimitWindowHours,
        follow_rate_limit_max_user: followRateLimitMaxUser,
        follow_rate_limit_max_moderator: followRateLimitMaxModerator,
        list_max_count_user: listMaxCountUser,
        list_max_count_moderator: listMaxCountModerator,
        list_member_max_user: listMemberMaxUser,
        list_member_max_moderator: listMemberMaxModerator,
        search_rate_limit_window_minutes: searchRateLimitWindowMinutes,
        search_rate_limit_max_user: searchRateLimitMaxUser,
        search_rate_limit_max_moderator: searchRateLimitMaxModerator,
        oembed_allowed_domains: oembedAllowedDomains,
      };
      // パスワード/シークレットは入力があったときだけ送る（未入力なら既存値を維持）。
      if (password) patch.smtp_password = password;
      if (turnstileSecretKey) patch.turnstile_secret_key = turnstileSecretKey;
      const s = await api.admin.updateSiteSettings(patch);
      setPasswordSet(s.smtp_password_set);
      setTurnstileSecretKeySet(s.turnstile_secret_key_set);
      setPassword("");
      setTurnstileSecretKey("");
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

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.mediaProxyTitle")}</div>
        <div className={styles.card}>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.mediaProxyUrlLabel")}
            <input className={styles.input} value={mediaProxyUrl} onChange={(e) => setMediaProxyUrl(e.target.value)} placeholder="https://media-proxy.example" />
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
          <p className={styles.hint}>{t("admin:siteSettingsPanel.turnstileHint")}</p>
        </div>

        <div style={{ fontWeight: 700, fontSize: "0.9rem", margin: "4px 0 8px" }}>{t("admin:siteSettingsPanel.roleRateLimitSectionTitle")}</div>
        <div className={styles.card}>
          <p className={styles.hint}>{t("admin:siteSettingsPanel.roleRateLimitHint")}</p>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.postRateLimitWindowMinutesLabel")}
            <input className={styles.input} type="number" min={1} value={postRateLimitWindowMinutes} onChange={(e) => setPostRateLimitWindowMinutes(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.postRateLimitMaxUserLabel")}
            <input className={styles.input} type="number" min={1} value={postRateLimitMaxUser} onChange={(e) => setPostRateLimitMaxUser(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.postRateLimitMaxModeratorLabel")}
            <input className={styles.input} type="number" min={1} value={postRateLimitMaxModerator} onChange={(e) => setPostRateLimitMaxModerator(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.followRateLimitWindowHoursLabel")}
            <input className={styles.input} type="number" min={1} value={followRateLimitWindowHours} onChange={(e) => setFollowRateLimitWindowHours(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.followRateLimitMaxUserLabel")}
            <input className={styles.input} type="number" min={1} value={followRateLimitMaxUser} onChange={(e) => setFollowRateLimitMaxUser(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.followRateLimitMaxModeratorLabel")}
            <input className={styles.input} type="number" min={1} value={followRateLimitMaxModerator} onChange={(e) => setFollowRateLimitMaxModerator(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.listMaxCountUserLabel")}
            <input className={styles.input} type="number" min={1} value={listMaxCountUser} onChange={(e) => setListMaxCountUser(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.listMaxCountModeratorLabel")}
            <input className={styles.input} type="number" min={1} value={listMaxCountModerator} onChange={(e) => setListMaxCountModerator(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.listMemberMaxUserLabel")}
            <input className={styles.input} type="number" min={1} value={listMemberMaxUser} onChange={(e) => setListMemberMaxUser(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.listMemberMaxModeratorLabel")}
            <input className={styles.input} type="number" min={1} value={listMemberMaxModerator} onChange={(e) => setListMemberMaxModerator(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.searchRateLimitWindowMinutesLabel")}
            <input className={styles.input} type="number" min={1} value={searchRateLimitWindowMinutes} onChange={(e) => setSearchRateLimitWindowMinutes(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.searchRateLimitMaxUserLabel")}
            <input className={styles.input} type="number" min={1} value={searchRateLimitMaxUser} onChange={(e) => setSearchRateLimitMaxUser(e.target.value)} />
          </label>
          <label className={styles.label}>
            {t("admin:siteSettingsPanel.searchRateLimitMaxModeratorLabel")}
            <input className={styles.input} type="number" min={1} value={searchRateLimitMaxModerator} onChange={(e) => setSearchRateLimitMaxModerator(e.target.value)} />
          </label>
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

        <button className={styles.btn} type="submit" disabled={saving}>
          {saving ? t("common:saving") : t("common:save")}
        </button>
      </form>
    </div>
  );
}
