import { FormEvent, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, ActorSuggestion, getErrorMessage, ListDetail, ListSummary } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useIsNarrowViewport } from "../hooks/useIsNarrowViewport";
import panel from "../components/common/Panel.module.css";
import styles from "./ListsSettings.module.css";

export default function ListsSettingsPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  // 狭幅では右ペインが無いため、メンバー編集パネルを中央ペインへ連続表示する。
  const isNarrow = useIsNarrowViewport();

  const [lists, setLists] = useState<ListSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ListDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  const [newName, setNewName] = useState("");
  const [newIsPublic, setNewIsPublic] = useState(false);
  const [creating, setCreating] = useState(false);

  const [editName, setEditName] = useState("");
  const [editIsPublic, setEditIsPublic] = useState(false);
  const [saving, setSaving] = useState(false);

  const [memberTarget, setMemberTarget] = useState("");
  const [addingMember, setAddingMember] = useState(false);
  const [memberError, setMemberError] = useState("");

  const [suggestions, setSuggestions] = useState<ActorSuggestion[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);

  function reloadLists() {
    return api.lists
      .list()
      .then(setLists)
      .catch((e) => setError(getErrorMessage(e)));
  }

  useEffect(() => {
    reloadLists().finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
      return;
    }
    let cancelled = false;
    setDetailLoading(true);
    api.lists
      .get(selectedId)
      .then((d) => {
        if (cancelled) return;
        setDetail(d);
        setEditName(d.name);
        setEditIsPublic(d.is_public);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setDetailLoading(false));
    setMemberTarget("");
    setSuggestions([]);
    return () => {
      cancelled = true;
    };
  }, [selectedId]);

  // メンバー追加入力のサジェスト（デバウンス300ms、DB上のアクターをユーザー名/表示名の部分一致で検索）。
  useEffect(() => {
    const q = memberTarget.trim();
    if (q.length === 0) {
      setSuggestions([]);
      return;
    }
    let cancelled = false;
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      api.actors
        .search(q, 8, controller.signal)
        .then((rows) => !cancelled && setSuggestions(rows))
        .catch(() => {});
    }, 300);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [memberTarget]);

  function selectSuggestion(s: ActorSuggestion) {
    setMemberTarget(s.target);
    setSuggestions([]);
    setShowSuggestions(false);
  }

  async function createList(e: FormEvent) {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    setCreating(true);
    setError("");
    try {
      const created = await api.lists.create(name, newIsPublic);
      setNewName("");
      setNewIsPublic(false);
      await reloadLists();
      setSelectedId(created.id);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setCreating(false);
    }
  }

  async function saveEdit(e: FormEvent) {
    e.preventDefault();
    if (!selectedId) return;
    const name = editName.trim();
    if (!name) return;
    setSaving(true);
    setError("");
    try {
      await api.lists.update(selectedId, name, editIsPublic);
      await reloadLists();
      const d = await api.lists.get(selectedId);
      setDetail(d);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  async function deleteList(id: string) {
    if (!confirm(t("lists:listsSettingsPage.deleteListConfirm"))) return;
    setError("");
    try {
      await api.lists.remove(id);
      if (selectedId === id) setSelectedId(null);
      await reloadLists();
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  async function addMember(e: FormEvent) {
    e.preventDefault();
    if (!selectedId) return;
    const target = memberTarget.trim();
    if (!target) return;
    setAddingMember(true);
    setMemberError("");
    try {
      await api.lists.addMember(selectedId, target);
      setMemberTarget("");
      setSuggestions([]);
      setShowSuggestions(false);
      const d = await api.lists.get(selectedId);
      setDetail(d);
      await reloadLists();
    } catch (err) {
      setMemberError(getErrorMessage(err));
    } finally {
      setAddingMember(false);
    }
  }

  async function removeMember(actorId: string) {
    if (!selectedId) return;
    setError("");
    try {
      await api.lists.removeMember(selectedId, actorId);
      const d = await api.lists.get(selectedId);
      setDetail(d);
      await reloadLists();
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={() => navigate(-1)}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("lists:listsSettingsPage.title")}</span>
      </header>

      {error && <p className={styles.error}>{error}</p>}

      <form className={styles.createForm} onSubmit={createList}>
        <input
          className={styles.input}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder={t("lists:listsSettingsPage.newListNamePlaceholder")}
          maxLength={100}
        />
        <label className={styles.checkboxLabel}>
          <input
            type="checkbox"
            checked={newIsPublic}
            onChange={(e) => setNewIsPublic(e.target.checked)}
          />
          {t("lists:listsSettingsPage.publicLabel")}
        </label>
        <button className={styles.save} type="submit" disabled={creating || !newName.trim()}>
          {creating ? t("lists:listsSettingsPage.creatingButton") : t("common:create")}
        </button>
      </form>

      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : lists.length === 0 ? (
        <p className={panel.message}>{t("lists:listsSettingsPage.noLists")}</p>
      ) : (
        <ul className={styles.listItems}>
          {lists.map((l) => (
            <li key={l.id}>
              <button
                className={`${styles.listItemBtn} ${selectedId === l.id ? styles.listItemActive : ""}`}
                onClick={() => setSelectedId(l.id)}
              >
                <span className={styles.listItemName}>{l.name}</span>
                <span className={styles.listItemMeta}>
                  {l.is_public ? t("lists:listsSettingsPage.isPublicLabel") : t("lists:listsSettingsPage.isPrivateLabel")} ・{" "}
                  {t("lists:listsSettingsPage.memberCount", { count: l.member_count })}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </>
  );

  const detailPanel = !selectedId ? (
    <p className={panel.message}>{t("lists:listsSettingsPage.selectListPrompt")}</p>
  ) : detailLoading || !detail ? (
    <p className={panel.message}>{t("common:loading")}</p>
  ) : (
    <>
      <div className={panel.rightHeader}>
        {t("lists:listsSettingsPage.editingHeader", { name: detail.name })}
      </div>

      <form className={styles.editForm} onSubmit={saveEdit}>
        <input
          className={styles.input}
          value={editName}
          onChange={(e) => setEditName(e.target.value)}
          maxLength={100}
        />
        <label className={styles.checkboxLabel}>
          <input
            type="checkbox"
            checked={editIsPublic}
            onChange={(e) => setEditIsPublic(e.target.checked)}
          />
          {t("lists:listsSettingsPage.publicLabel")}
        </label>
        <button className={styles.save} type="submit" disabled={saving || !editName.trim()}>
          {saving ? t("common:saving") : t("common:save")}
        </button>
        <button type="button" className={styles.dangerBtn} onClick={() => deleteList(detail.id)}>
          {t("common:delete")}
        </button>
      </form>

      <form className={styles.memberForm} onSubmit={addMember}>
        <div className={styles.memberInputWrap}>
          <input
            className={styles.input}
            value={memberTarget}
            onChange={(e) => {
              setMemberTarget(e.target.value);
              setShowSuggestions(true);
            }}
            onFocus={() => setShowSuggestions(true)}
            placeholder={t("lists:listsSettingsPage.memberSearchPlaceholder")}
            autoComplete="off"
          />
          {showSuggestions && suggestions.length > 0 && (
            <ul className={styles.suggestList}>
              {suggestions.map((s) => (
                <li key={s.actor_id}>
                  <button
                    type="button"
                    className={styles.suggestItem}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => selectSuggestion(s)}
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
        <button className={styles.save} type="submit" disabled={addingMember || !memberTarget.trim()}>
          {addingMember ? t("lists:listsSettingsPage.addingMemberButton") : t("lists:listsSettingsPage.addMemberButton")}
        </button>
      </form>
      {memberError && <p className={styles.error}>{memberError}</p>}

      <ul className={styles.memberList}>
        {detail.members.map((m) => (
          <li key={m.actor_id} className={styles.memberRow}>
            <span className={styles.memberAvatar}>
              {m.avatar_url ? <img src={m.avatar_url} alt="" /> : <span>{(m.display_name || m.username)[0]?.toUpperCase()}</span>}
            </span>
            <span className={styles.memberName}>
              {m.display_name || m.username}
              <span className={styles.memberHandle}>
                @{m.username}
                {m.domain ? `@${m.domain}` : ""}
              </span>
            </span>
            <span className={styles.memberType}>{m.actor_type}</span>
            <button className={styles.removeBtn} onClick={() => removeMember(m.actor_id)}>
              {t("common:delete")}
            </button>
          </li>
        ))}
        {detail.members.length === 0 && <p className={panel.message}>{t("lists:listsSettingsPage.noMembers")}</p>}
      </ul>
    </>
  );

  return (
    <AppShell
      center={
        <>
          {center}
          {isNarrow && <div className={styles.narrowDetail}>{detailPanel}</div>}
        </>
      }
      right={!isNarrow ? detailPanel : null}
    />
  );
}
