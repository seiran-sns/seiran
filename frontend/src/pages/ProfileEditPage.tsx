import { ChangeEvent, FormEvent, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, DriveFile, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useAuth } from "../contexts/AuthContext";
import panel from "../components/common/Panel.module.css";
import styles from "./ProfileEdit.module.css";

export default function ProfileEditPage() {
  const { user } = useAuth();
  const navigate = useNavigate();

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  const [displayName, setDisplayName] = useState("");
  const [bio, setBio] = useState("");
  const [avatar, setAvatar] = useState<DriveFile | null>(null);
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
        ...(avatar ? { avatar_media_id: Number(avatar.id) } : {}),
      });
      setSaved(true);
      setTimeout(() => navigate(`/profile?q=${encodeURIComponent(user?.username ?? "")}`), 500);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSaving(false);
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
              {avatar ? (
                <img src={avatar.url} alt="" />
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

          <p className={styles.hint}>
            プロフィールのキーバリュー項目（リンク等）は今後対応予定です。
          </p>

          <button className={styles.save} type="submit" disabled={saving}>
            {saving ? "保存中..." : "保存"}
          </button>
        </form>
      )}
    </>
  );

  return <AppShell center={center} />;
}
