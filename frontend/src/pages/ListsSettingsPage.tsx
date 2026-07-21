import { useNavigate } from "react-router-dom";
import AppShell from "../components/layout/AppShell";
import ListsPanel from "../components/lists/ListsPanel";
import ListDetailPanel from "../components/lists/ListDetailPanel";
import { useIsNarrowViewport } from "../hooks/useIsNarrowViewport";
import { useListsSettings } from "../hooks/useListsSettings";
import styles from "./ListsSettings.module.css";

export default function ListsSettingsPage() {
  const navigate = useNavigate();

  // 狭幅では右ペインが無いため、メンバー編集パネルを中央ペインへ連続表示する。
  const isNarrow = useIsNarrowViewport();

  const {
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
  } = useListsSettings();

  const center = (
    <ListsPanel
      onBack={() => navigate(-1)}
      error={error}
      lists={lists}
      loading={loading}
      selectedId={selectedId}
      onSelect={setSelectedId}
      newName={newName}
      onNewNameChange={setNewName}
      newIsPublic={newIsPublic}
      onNewIsPublicChange={setNewIsPublic}
      creating={creating}
      onCreateSubmit={createList}
    />
  );

  const detailPanel = (
    <ListDetailPanel
      selectedId={selectedId}
      detail={detail}
      detailLoading={detailLoading}
      editName={editName}
      onEditNameChange={setEditName}
      editIsPublic={editIsPublic}
      onEditIsPublicChange={setEditIsPublic}
      saving={saving}
      onSaveEdit={saveEdit}
      onDelete={deleteList}
      memberTarget={memberTarget}
      onMemberTargetChange={setMemberTarget}
      addingMember={addingMember}
      memberError={memberError}
      onAddMember={addMember}
      onRemoveMember={removeMember}
      suggestions={suggestions}
      showSuggestions={showSuggestions}
      onShowSuggestions={setShowSuggestions}
      onSelectSuggestion={selectSuggestion}
    />
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
