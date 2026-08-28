import { ChangeEvent, FormEvent, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ActorSuggestion, AlsoKnownAsItem, api, DriveFile, getErrorMessage, ProfileField } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useAuth } from "../contexts/AuthContext";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import panel from "../components/common/Panel.module.css";
import styles from "./ProfileEdit.module.css";

/** プロフィール編集フォームで扱う固定スロット数（#62、Mastodon のデフォルト4件に合わせる）。 */
const PROFILE_FIELD_SLOTS = 4;

function emptyProfileFields(): ProfileField[] {
  return Array.from({ length: PROFILE_FIELD_SLOTS }, () => ({ name: "", value: "" }));
}

export default function ProfileEditPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const navigate = useNavigate();
  const goBack = useGoBack();

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  const [displayName, setDisplayName] = useState("");
  const [bio, setBio] = useState("");
  const [birthday, setBirthday] = useState("");
  const [birthdayPublic, setBirthdayPublic] = useState(false);
  const [profileFields, setProfileFields] = useState<ProfileField[]>(emptyProfileFields());
  const [akaItems, setAkaItems] = useState<AlsoKnownAsItem[]>([]);
  const [akaTarget, setAkaTarget] = useState("");
  const [addingAka, setAddingAka] = useState(false);
  const [akaError, setAkaError] = useState("");
  const [akaSuggestions, setAkaSuggestions] = useState<ActorSuggestion[]>([]);
  const [showAkaSuggestions, setShowAkaSuggestions] = useState(false);
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
        setBirthday(p.birthday ?? "");
        setBirthdayPublic(p.birthday_public ?? false);
        setCurrentAvatarUrl(p.avatar_url ?? null);
        const slots = emptyProfileFields();
        p.profile_fields.slice(0, PROFILE_FIELD_SLOTS).forEach((f, i) => { slots[i] = f; });
        setProfileFields(slots);
        setAkaItems(p.also_known_as);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [user]);

  // 「別のアカウント」追加入力のサジェスト（デバウンス300ms）。lists機能のメンバー追加
  // （`useListsSettings.ts`）と同じ仕組みを流用している。
  useEffect(() => {
    const q = akaTarget.trim();
    if (q.length === 0) {
      setAkaSuggestions([]);
      return;
    }
    let cancelled = false;
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      api.actors
        .search(q, 8, controller.signal)
        .then((rows) => !cancelled && setAkaSuggestions(rows))
        .catch(() => {});
    }, 300);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [akaTarget]);

  function selectAkaSuggestion(s: ActorSuggestion) {
    setAkaTarget(s.target);
    setAkaSuggestions([]);
    setShowAkaSuggestions(false);
  }

  // ボタン型は button（submit ではない）: この節はプロフィール編集の外側 <form> の内側に
  // あるため、ここに <form> を入れ子にすると（HTML仕様上不正）ブラウザのネイティブ送信
  // （ページリロード）が発火して外側の onSubmit を巻き込み、意図しない挙動になる
  // （実際に発生した不具合: 「追加」を押しても何も起きずページがリロードされたように見える）。
  async function addAka() {
    const target = akaTarget.trim();
    if (!target) return;
    setAddingAka(true);
    setAkaError("");
    try {
      setAkaItems(await api.alsoKnownAs.add(target));
      setAkaTarget("");
      setAkaSuggestions([]);
      setShowAkaSuggestions(false);
    } catch (err) {
      setAkaError(getErrorMessage(err));
    } finally {
      setAddingAka(false);
    }
  }

  async function removeAka(actorId: string) {
    setAkaError("");
    try {
      setAkaItems(await api.alsoKnownAs.remove(actorId));
    } catch (err) {
      setAkaError(getErrorMessage(err));
    }
  }

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
        birthday: birthday || null,
        birthday_public: birthdayPublic,
      });
      setSaved(true);
      setTimeout(() => navigate(`/@${user?.username ?? ""}`), 500);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("profile:profileEditPage.title")}</span>
      </header>

      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : (
        <form className={styles.form} onSubmit={save}>
          {error && <p className={styles.error}>{error}</p>}
          {saved && <p className={styles.success}>{t("profile:profileEditPage.savedMessage")}</p>}

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
              {uploading ? t("profile:profileEditPage.uploadingAvatar") : t("profile:profileEditPage.changeAvatarButton")}
            </button>
          </div>

          <label className={styles.label}>
            {t("profile:profileEditPage.displayNameLabel")}
            <input
              className={styles.input}
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder={user?.username}
              maxLength={80}
            />
          </label>

          <label className={styles.label}>
            {t("profile:profileEditPage.bioLabel")}
            <textarea
              className={styles.textarea}
              value={bio}
              onChange={(e) => setBio(e.target.value)}
              rows={5}
              placeholder={t("profile:profileEditPage.bioPlaceholder")}
            />
          </label>

          <label className={styles.label}>
            {t("profile:profileEditPage.birthdayLabel")}
            <input
              className={styles.input}
              type="date"
              value={birthday}
              onChange={(e) => setBirthday(e.target.value)}
            />
          </label>

          <label className={styles.checkboxLabel}>
            <input
              type="checkbox"
              checked={birthdayPublic}
              onChange={(e) => setBirthdayPublic(e.target.checked)}
            />
            {t("profile:profileEditPage.birthdayPublicLabel")}
          </label>

          <div className={styles.fieldsSection}>
            <p className={styles.fieldsLabel}>
              {t("profile:profileEditPage.fieldsLabel", { count: PROFILE_FIELD_SLOTS })}
            </p>
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
                  placeholder={t("profile:profileEditPage.fieldNamePlaceholder")}
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
                  placeholder={t("profile:profileEditPage.fieldValuePlaceholder")}
                  maxLength={255}
                />
              </div>
            ))}
          </div>

          <div className={styles.akaSection}>
            <p className={styles.akaLabel}>{t("profile:profileEditPage.akaLabel")}</p>
            <p className={styles.hint}>{t("profile:profileEditPage.akaVerifiedExplanation")}</p>

            <div className={styles.akaForm}>
              <div className={styles.akaInputWrap}>
                <input
                  className={styles.input}
                  value={akaTarget}
                  onChange={(e) => {
                    setAkaTarget(e.target.value);
                    setShowAkaSuggestions(true);
                  }}
                  onFocus={() => setShowAkaSuggestions(true)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      if (!addingAka && akaTarget.trim()) void addAka();
                    }
                  }}
                  placeholder={t("profile:profileEditPage.akaSearchPlaceholder")}
                  autoComplete="off"
                />
                {showAkaSuggestions && akaSuggestions.length > 0 && (
                  <ul className={styles.suggestList}>
                    {akaSuggestions.map((s) => (
                      <li key={s.actor_id}>
                        <button
                          type="button"
                          className={styles.suggestItem}
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => selectAkaSuggestion(s)}
                        >
                          <span className={styles.suggestAvatar}>
                            {s.avatar_url ? (
                              <img src={s.avatar_url} alt="" />
                            ) : (
                              <span>{(s.display_name || s.username)[0]?.toUpperCase()}</span>
                            )}
                          </span>
                          <span className={styles.suggestName}>
                            {s.display_name || s.username}
                            <span className={styles.suggestHandle}>
                              @{s.username}
                              {s.domain ? `@${s.domain}` : ""}
                            </span>
                          </span>
                          <span className={styles.suggestType}>{s.actor_type}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
              <button
                className={styles.save}
                type="button"
                onClick={() => void addAka()}
                disabled={addingAka || !akaTarget.trim()}
              >
                {addingAka ? t("profile:profileEditPage.addingAkaButton") : t("profile:profileEditPage.addAkaButton")}
              </button>
            </div>
            {akaError && <p className={styles.error}>{akaError}</p>}

            <ul className={styles.akaList}>
              {akaItems.map((item) => (
                <li key={item.actor_id} className={styles.akaRow}>
                  <span className={styles.akaAvatar}>
                    {item.avatar_url ? (
                      <img src={item.avatar_url} alt="" />
                    ) : (
                      <span>{(item.display_name || item.username)[0]?.toUpperCase()}</span>
                    )}
                  </span>
                  <span className={styles.akaName}>
                    {item.display_name || item.username}
                    <span className={styles.akaHandle}>
                      @{item.username}
                      {item.domain ? `@${item.domain}` : ""}
                    </span>
                  </span>
                  <span className={styles.akaType}>{item.actor_type}</span>
                  {item.verified && (
                    <span className={styles.akaVerified} title={t("profile:profileEditPage.akaVerifiedExplanation")}>
                      ✅
                    </span>
                  )}
                  <button type="button" className={styles.removeBtn} onClick={() => removeAka(item.actor_id)}>
                    {t("common:delete")}
                  </button>
                </li>
              ))}
              {akaItems.length === 0 && <p className={styles.hint}>{t("profile:profileEditPage.noAka")}</p>}
            </ul>
          </div>

          <button className={styles.save} type="submit" disabled={saving}>
            {saving ? t("common:saving") : t("common:save")}
          </button>
        </form>
      )}
    </>
  );

  return <AppShell center={center} />;
}
