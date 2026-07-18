import { ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { Note } from "../../api/client";
import Modal from "../common/Modal";
import PostComposer from "../note/PostComposer";
import LeftNav from "./LeftNav";
import styles from "./AppShell.module.css";

interface AppShellProps {
  /** 中央ペイン（メインコンテンツストリーム）。 */
  center: ReactNode;
  /** 右ペイン（動的コンテキスト領域）。省略時は非表示。 */
  right?: ReactNode;
  /** 投稿完了時のコールバック（ホーム画面が新規ノートを先頭に差し込むのに使う）。 */
  onPosted?: (note: Note) => void;
}

export default function AppShell({ center, right, onPosted }: AppShellProps) {
  const { t } = useTranslation();
  const [composeOpen, setComposeOpen] = useState(false);

  return (
    <div className={styles.shell}>
      <LeftNav onCompose={() => setComposeOpen(true)} />

      <main className={styles.center}>{center}</main>

      <aside className={styles.right}>{right}</aside>

      <Modal open={composeOpen} onClose={() => setComposeOpen(false)} title={t("nav:appShell.composeModalTitle")}>
        <PostComposer
          autoFocus
          onPosted={(note) => {
            setComposeOpen(false);
            onPosted?.(note);
          }}
        />
      </Modal>
    </div>
  );
}
