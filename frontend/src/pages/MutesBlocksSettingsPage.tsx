import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { api, getErrorMessage, MutedOrBlockedActor } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import Avatar from "../components/note/Avatar";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useToast } from "../contexts/ToastContext";
import { profilePath, profileQuery } from "../lib/format";
import panel from "../components/common/Panel.module.css";
import styles from "./MutesBlocksSettings.module.css";

/** メインメニュー「設定」内のミュート・ブロック管理（#55）。対象者一覧とその解除。 */
export default function MutesBlocksSettingsPage() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const goBack = useGoBack();

  const [tab, setTab] = useState(0);
  const [mutes, setMutes] = useState<MutedOrBlockedActor[] | null>(null);
  const [blocks, setBlocks] = useState<MutedOrBlockedActor[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [actionLoadingId, setActionLoadingId] = useState<string | null>(null);

  const loadAll = useCallback(() => {
    setLoading(true);
    Promise.all([api.mutes.list(), api.blocks.list()])
      .then(([m, b]) => {
        setMutes(m);
        setBlocks(b);
      })
      .catch((e) => showError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, [showError]);

  useEffect(() => {
    loadAll();
  }, [loadAll]);

  async function unmute(actor: MutedOrBlockedActor) {
    setActionLoadingId(actor.actor_id);
    try {
      await api.mutes.delete(profileQuery(actor.username, actor.domain));
      setMutes((prev) => prev?.filter((a) => a.actor_id !== actor.actor_id) ?? null);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setActionLoadingId(null);
    }
  }

  async function unblock(actor: MutedOrBlockedActor) {
    setActionLoadingId(actor.actor_id);
    try {
      await api.blocks.delete(profileQuery(actor.username, actor.domain));
      setBlocks((prev) => prev?.filter((a) => a.actor_id !== actor.actor_id) ?? null);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setActionLoadingId(null);
    }
  }

  const items = tab === 0 ? mutes : blocks;
  const emptyMessage = tab === 0 ? t("account:mutesBlocks.noMutes") : t("account:mutesBlocks.noBlocks");

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:mutesBlocks.title")}</span>
      </header>

      <Tabs
        tabs={[t("account:mutesBlocks.mutesTab"), t("account:mutesBlocks.blocksTab")]}
        active={tab}
        onChange={setTab}
      />

      {loading && <p className={panel.message}>{t("common:loading")}</p>}
      {!loading && items && items.length === 0 && <p className={panel.message}>{emptyMessage}</p>}

      {!loading && items && items.length > 0 && (
        <ul className={styles.list}>
          {items.map((actor) => (
            <li key={actor.actor_id} className={styles.row}>
              <Link to={profilePath(actor.username, actor.domain)} className={styles.actorLink}>
                <Avatar url={actor.avatar_url} name={actor.display_name || actor.username} size={40} />
                <div className={styles.names}>
                  <span className={styles.displayName}>{actor.display_name || actor.username}</span>
                  <span className={styles.acct}>
                    @{actor.username}
                    {actor.domain && actor.domain !== window.location.hostname && `@${actor.domain}`}
                  </span>
                </div>
              </Link>
              <button
                type="button"
                className={styles.releaseBtn}
                disabled={actionLoadingId === actor.actor_id}
                onClick={() => (tab === 0 ? unmute(actor) : unblock(actor))}
              >
                {tab === 0 ? t("account:mutesBlocks.unmuteButton") : t("account:mutesBlocks.unblockButton")}
              </button>
            </li>
          ))}
        </ul>
      )}
    </>
  );

  return <AppShell center={center} />;
}
