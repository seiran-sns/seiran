import { FormEvent, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ActorSuggestion, getErrorMessage, ListDetail, ListSummary } from "../api/client";

/**
 * ListsSettingsPage が必要とするデータ取得・ミューテーション（リスト一覧取得、作成/改名/削除、
 * メンバー追加/削除）と、それに紐づくローカル state をまとめて提供するフック。
 * 呼び出し側は返された state とハンドラを JSX に配線するだけでよく、表示ロジックに専念できる。
 */
export function useListsSettings() {
  const { t } = useTranslation();

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

  return {
    lists,
    loading,
    error,

    selectedId,
    setSelectedId,
    detail,
    detailLoading,

    newName,
    setNewName,
    newIsPublic,
    setNewIsPublic,
    creating,
    createList,

    editName,
    setEditName,
    editIsPublic,
    setEditIsPublic,
    saving,
    saveEdit,

    deleteList,

    memberTarget,
    setMemberTarget,
    addingMember,
    memberError,
    addMember,
    removeMember,

    suggestions,
    showSuggestions,
    setShowSuggestions,
    selectSuggestion,
  };
}
