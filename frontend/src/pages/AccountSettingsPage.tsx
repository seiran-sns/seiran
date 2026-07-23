import { FormEvent, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useAuth } from "../contexts/AuthContext";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import panel from "../components/common/Panel.module.css";
import styles from "./AccountSettings.module.css";

/** メインメニュー「設定」内のアカウント設定（#55）。DID確認・パスワード変更・退会（旧プロフィール編集画面から移動）。 */
export default function AccountSettingsPage() {
  const { t } = useTranslation();
  const { user, logout } = useAuth();
  const navigate = useNavigate();
  const goBack = useGoBack();

  const [did, setDid] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(true);

  // パスワード変更
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [newPasswordConfirm, setNewPasswordConfirm] = useState("");
  const [changingPassword, setChangingPassword] = useState(false);
  const [passwordError, setPasswordError] = useState("");
  const [passwordSaved, setPasswordSaved] = useState(false);

  // 退会
  const [withdrawHandle, setWithdrawHandle] = useState("");
  const [withdrawing, setWithdrawing] = useState(false);
  const [withdrawError, setWithdrawError] = useState("");
  const [showWithdrawForm, setShowWithdrawForm] = useState(false);

  useEffect(() => {
    if (!user) return;
    let cancelled = false;
    api.users
      .profile(user.username)
      .then((p) => !cancelled && setDid(p.at_did))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [user]);

  async function changePassword(e: FormEvent) {
    e.preventDefault();
    setPasswordError("");
    setPasswordSaved(false);
    if (newPassword !== newPasswordConfirm) {
      setPasswordError(t("account:accountSettings.passwordMismatch"));
      return;
    }
    setChangingPassword(true);
    try {
      await api.account.changePassword(currentPassword, newPassword);
      setPasswordSaved(true);
      setCurrentPassword("");
      setNewPassword("");
      setNewPasswordConfirm("");
    } catch (err) {
      setPasswordError(getErrorMessage(err));
    } finally {
      setChangingPassword(false);
    }
  }

  async function withdraw(e: FormEvent) {
    e.preventDefault();
    if (!confirm(t("profile:profileEditPage.withdrawConfirm"))) return;
    setWithdrawing(true);
    setWithdrawError("");
    try {
      await api.account.withdraw(withdrawHandle.trim());
      logout();
      navigate("/login");
    } catch (err) {
      setWithdrawError(getErrorMessage(err));
    } finally {
      setWithdrawing(false);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:accountSettings.title")}</span>
      </header>

      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : (
        <div className={styles.section}>
          <h3 className={styles.sectionTitle}>{t("account:accountSettings.identityTitle")}</h3>
          <div className={styles.identityRow}>
            <span className={styles.identityLabel}>{t("account:accountSettings.emailLabel")}</span>
            <span className={styles.identityValue}>{user?.email}</span>
          </div>
          {did && (
            <div className={styles.identityRow}>
              <span className={styles.identityLabel}>DID</span>
              <span className={styles.identityValue}>{did}</span>
            </div>
          )}
        </div>
      )}

      <form className={styles.section} onSubmit={changePassword}>
        <h3 className={styles.sectionTitle}>{t("account:accountSettings.passwordTitle")}</h3>
        {passwordError && <p className={styles.error}>{passwordError}</p>}
        {passwordSaved && <p className={styles.success}>{t("account:accountSettings.passwordSaved")}</p>}
        <label className={styles.label}>
          {t("account:accountSettings.currentPasswordLabel")}
          <input
            className={styles.input}
            type="password"
            value={currentPassword}
            onChange={(e) => setCurrentPassword(e.target.value)}
            autoComplete="current-password"
            required
          />
        </label>
        <label className={styles.label}>
          {t("account:accountSettings.newPasswordLabel")}
          <input
            className={styles.input}
            type="password"
            value={newPassword}
            onChange={(e) => setNewPassword(e.target.value)}
            autoComplete="new-password"
            minLength={8}
            required
          />
        </label>
        <label className={styles.label}>
          {t("account:accountSettings.newPasswordConfirmLabel")}
          <input
            className={styles.input}
            type="password"
            value={newPasswordConfirm}
            onChange={(e) => setNewPasswordConfirm(e.target.value)}
            autoComplete="new-password"
            minLength={8}
            required
          />
        </label>
        <button className={styles.save} type="submit" disabled={changingPassword}>
          {changingPassword ? t("common:saving") : t("account:accountSettings.changePasswordButton")}
        </button>
      </form>

      {/* 退会（#29、旧プロフィール編集画面からこちらへ移動、#55） */}
      <div className={styles.dangerZone}>
        <h3 className={styles.dangerTitle}>{t("profile:profileEditPage.dangerZoneTitle")}</h3>
        {!showWithdrawForm ? (
          <button className={styles.dangerBtn} onClick={() => setShowWithdrawForm(true)}>
            {t("profile:profileEditPage.withdrawButton")}
          </button>
        ) : (
          <form className={styles.withdrawForm} onSubmit={withdraw}>
            <p className={styles.dangerHint}>
              {t("profile:profileEditPage.withdrawHint.body")}
              {t("profile:profileEditPage.withdrawHint.handlePrefix")}
              <strong>@{user?.username}</strong>
              {t("profile:profileEditPage.withdrawHint.handleSuffix")}
            </p>
            {withdrawError && <p className={styles.error}>{withdrawError}</p>}
            <input
              className={styles.input}
              value={withdrawHandle}
              onChange={(e) => setWithdrawHandle(e.target.value)}
              placeholder={user?.username ?? ""}
              disabled={withdrawing}
            />
            <div className={styles.withdrawActions}>
              <button
                type="button"
                className={styles.ghost}
                onClick={() => {
                  setShowWithdrawForm(false);
                  setWithdrawError("");
                }}
                disabled={withdrawing}
              >
                {t("common:cancel")}
              </button>
              <button type="submit" className={styles.dangerBtn} disabled={withdrawing || !withdrawHandle.trim()}>
                {withdrawing ? t("profile:profileEditPage.withdrawing") : t("profile:profileEditPage.withdrawSubmit")}
              </button>
            </div>
          </form>
        )}
      </div>
    </>
  );

  return <AppShell center={center} />;
}
