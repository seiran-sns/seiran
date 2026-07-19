import { createContext, useContext, useState } from "react";
import { useTranslation } from "react-i18next";
import { Note } from "../api/client";
import Modal from "../components/common/Modal";
import PostComposer from "../components/note/PostComposer";

/**
 * 投稿コンポーザをグローバルに開くためのコンテキスト（issue #23）。
 * 任意のポストの「返信」ボタンから `openReply(note)` を呼ぶとモーダルが開く。
 * ハッシュタグ画面の「このハッシュタグでポスト」等、本文を指定して素の投稿
 * ダイアログを開きたい箇所からは `openCompose(initialText)` を呼ぶ。
 */
type ComposerState = { mode: "reply"; target: Note } | { mode: "compose"; initialText: string } | null;

interface ComposerContextValue {
  openReply: (target: Note) => void;
  openCompose: (initialText?: string) => void;
}

const ComposerContext = createContext<ComposerContextValue>({ openReply: () => {}, openCompose: () => {} });

export function ComposerProvider({ children }: { children: React.ReactNode }) {
  const { t } = useTranslation();
  const [state, setState] = useState<ComposerState>(null);

  const openReply = (target: Note) => setState({ mode: "reply", target });
  const openCompose = (initialText = "") => setState({ mode: "compose", initialText });
  const close = () => setState(null);

  return (
    <ComposerContext.Provider value={{ openReply, openCompose }}>
      {children}
      <Modal
        open={state !== null}
        onClose={close}
        title={state?.mode === "reply" ? "返信" : t("nav:appShell.composeModalTitle")}
      >
        {state?.mode === "reply" && (
          <PostComposer key={state.target.id} autoFocus replyTo={state.target} onPosted={close} />
        )}
        {state?.mode === "compose" && (
          <PostComposer key={state.initialText} autoFocus initialText={state.initialText} onPosted={close} />
        )}
      </Modal>
    </ComposerContext.Provider>
  );
}

export function useComposer() {
  return useContext(ComposerContext);
}
