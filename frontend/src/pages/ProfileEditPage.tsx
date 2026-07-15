import { ChangeEvent, FormEvent, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, DriveFile, getErrorMessage, ApiError, ProfileField } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useAuth } from "../contexts/AuthContext";
import panel from "../components/common/Panel.module.css";
import styles from "./ProfileEdit.module.css";

/** プロフィール編集フォームで扱う固定スロット数（#62、Mastodon のデフォルト4件に合わせる）。 */
const PROFILE_FIELD_SLOTS = 4;

function emptyProfileFields(): ProfileField[] {
  return Array.from({ length: PROFILE_FIELD_SLOTS }, () => ({ name: "", value: "" }));
}

const WITHDRAW_ERROR: Record<string, string> = {
  CONFIRM_HANDLE_MISMATCH: "ハンドルが一致しません",
  ALREADY_WITHDRAWN: "すでに退会済みです",
  ACTOR_NOT_FOUND: "アクターが見つかりません",
};

export default function ProfileEditPage() {
  const { user } = useAuth();
  const navigate = useNavigate();

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  // 退会フォーム
  const [withdrawHandle, setWithdrawHandle] = useState("");
  const [withdrawing, setWithdrawing] = useState(false);
  const [withdrawError, setWithdrawError] = useState("");
  const [showWithdrawForm, setShowWithdrawForm] = useState(false);

  const [displayName, setDisplayName] = useState("");
  const [bio, setBio] = useState("");
  const [profileFields, setProfileFields] = useState<ProfileField[]>(emptyProfileFields());
  const [avatar, setAvatar] = useState<DriveFile | null>(null);
  /** 既存のアイコンURL（未変更時のプレビュー用）。新規アップロード後は avatar.url を優先する。 */
  const [currentAvatarUrl, setCurrentAvatarUrl] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!user) return;
    let cancelled = false;
    api.users
      .profile(user.username)
      .then((p) => {
        if (cancelled) return;
        setDisplayName(p.display_name ?? "");
        setBio(p.bio ?? "");
        setCurrentAvatarUrl(p.avatar_url ?? null);
        const slots = emptyProfileFields();
        p.profile_fields.slice(0, PROFILE_FIELD_SLOTS).forEach((f, i) => { slots[i] = f; });
        setProfileFields(slots);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [user]);

  async function onAvatar(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setUploading(true);
    setError("");
    try {
      setAvatar(await api.media.upload(file, "avatar"));
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setUploading(false);
    }
  }

  async function save(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      await api.users.updateProfile({
        display_name: displayName,
        bio,
        ...(avatar ? { avatar_media_id: avatar.id } : {}),
        profile_fields: profileFields.filter((f) => f.name.trim() && f.value.trim()),
      });
      setSaved(true);
      setTimeout(() => navigate(`/@${user?.username ?? ""}`), 500);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  async function withdraw(e: FormEvent) {
    e.preventDefault();
    if (!confirm("退会すると元に戻せません。本当に退会しますか？")) return;
    setWithdrawing(true);
    setWithdrawError("");
    try {
      await api.account.withdraw(withdrawHandle.trim());
      localStorage.removeItem("seiran_token");
      navigate("/login");
    } catch (err) {
      if (err instanceof ApiError) {
        setWithdrawError(WITHDRAW_ERROR[err.code] ?? `エラー (${err.code})`);
      } else {
        setWithdrawError(getErrorMessage(err));
      }
    } finally {
      setWithdrawing(false);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={() => navigate(-1)}>
          ← 戻る
        </button>
        <span className={panel.title}>プロフィール編集</span>
      </header>

      {loading ? (
        <p className={panel.message}>読み込み中...</p>
      ) : (
        <form className={styles.form} onSubmit={save}>
          {error && <p className={styles.error}>{error}</p>}
          {saved && <p className={styles.success}>保存しました。</p>}

          <div className={styles.avatarRow}>
            <div className={styles.avatarPreview}>
              {avatar || currentAvatarUrl ? (
                <img src={avatar ? avatar.url : currentAvatarUrl!} alt="" />
              ) : (
                <span>{(displayName || user?.username || "?")[0]?.toUpperCase()}</span>
              )}
            </div>
            <input ref={fileRef} type="file" accept="image/*" style={{ display: "none" }} onChange={onAvatar} />
            <button type="button" className={styles.ghost} onClick={() => fileRef.current?.click()} disabled={uploading}>
              {uploading ? "アップロード中..." : "アイコンを変更"}
            </button>
          </div>

          <label className={styles.label}>
            表示名
            <input
              className={styles.input}
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder={user?.username}
              maxLength={80}
            />
          </label>

          <label className={styles.label}>
            自己紹介
            <textarea
              className={styles.textarea}
              value={bio}
              onChange={(e) => setBio(e.target.value)}
              rows={5}
              placeholder="あなたについて教えてください"
            />
          </label>

          <div className={styles.fieldsSection}>
            <p className={styles.fieldsLabel}>プロフィール項目（リンク等、最大{PROFILE_FIELD_SLOTS}件）</p>
            {profileFields.map((field, i) => (
              <div className={styles.fieldRow} key={i}>
                <input
                  className={`${styles.input} ${styles.fieldName}`}
                  value={field.name}
                  onChange={(e) => {
                    const next = [...profileFields];
                    next[i] = { ...next[i], name: e.target.value };
                    setProfileFields(next);
                  }}
                  placeholder="ラベル（例: サイト）"
                  maxLength={50}
                />
                <input
                  className={styles.input}
                  value={field.value}
                  onChange={(e) => {
                    const next = [...profileFields];
                    next[i] = { ...next[i], value: e.target.value };
                    setProfileFields(next);
                  }}
                  placeholder="値（例: https://example.com）"
                  maxLength={255}
                />
              </div>
            ))}
          </div>

          <button className={styles.save} type="submit" disabled={saving}>
            {saving ? "保存中..." : "保存"}
          </button>
        </form>
      )}

      {/* 退会 */}
      <div className={styles.dangerZone}>
        <h3 className={styles.dangerTitle}>危険な操作</h3>
        {!showWithdrawForm ? (
          <button className={styles.dangerBtn} onClick={() => setShowWithdrawForm(true)}>
            このアカウントを退会する
          </button>
        ) : (
          <form className={styles.withdrawForm} onSubmit={withdraw}>
            <p className={styles.dangerHint}>
              退会すると投稿が削除され、フォロー関係が解除されます。この操作は取り消せません。
              確認のため、自分のハンドル（<strong>@{user?.username}</strong>）を入力してください。
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
                onClick={() => { setShowWithdrawForm(false); setWithdrawError(""); }}
                disabled={withdrawing}
              >
                キャンセル
              </button>
              <button
                type="submit"
                className={styles.dangerBtn}
                disabled={withdrawing || !withdrawHandle.trim()}
              >
                {withdrawing ? "処理中..." : "退会する"}
              </button>
            </div>
          </form>
        )}
      </div>
    </>
  );

  return <AppShell center={center} />;
}
