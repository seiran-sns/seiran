import { createContext, useContext, useState } from "react";
import { Note } from "../api/client";
import Modal from "../components/common/Modal";
import PostComposer from "../components/note/PostComposer";

/**
 * 返信コンポーザをグローバルに開くためのコンテキスト（issue #23）。
 * 任意のポストの「返信」ボタンから `openReply(note)` を呼ぶとモーダルが開く。
 */
interface ComposerContextValue {
  openReply: (target: Note) => void;
}

const ComposerContext = createContext<ComposerContextValue>({ openReply: () => {} });

export function ComposerProvider({ children }: { children: React.ReactNode }) {
  const [replyTarget, setReplyTarget] = useState<Note | null>(null);

  return (
    <ComposerContext.Provider value={{ openReply: setReplyTarget }}>
      {children}
      <Modal open={replyTarget !== null} onClose={() => setReplyTarget(null)} title="返信">
        {replyTarget && (
          <PostComposer key={replyTarget.id} autoFocus replyTo={replyTarget} onPosted={() => setReplyTarget(null)} />
        )}
      </Modal>
    </ComposerContext.Provider>
  );
}

export function useComposer() {
  return useContext(ComposerContext);
}
