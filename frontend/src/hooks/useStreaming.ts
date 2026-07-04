import { useEffect, useRef } from "react";
import { getToken, noteFromStream, Note } from "../api/client";

/**
 * リアルタイム更新の WebSocket 接続（#37）。
 * `note` イベントを受け取ると `onNote` を呼ぶ。切断時は自動再接続する。
 */
export function useStreaming(onNote: (n: Note) => void) {
  const onNoteRef = useRef(onNote);
  onNoteRef.current = onNote;

  useEffect(() => {
    const token = getToken();
    if (!token) return;

    let closed = false;
    let ws: WebSocket | null = null;
    let retry: number | null = null;

    function connect() {
      if (closed) return;
      const proto = location.protocol === "https:" ? "wss" : "ws";
      ws = new WebSocket(`${proto}://${location.host}/api/streaming?token=${encodeURIComponent(token!)}`);
      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          if (msg.type === "note" && msg.body) onNoteRef.current(noteFromStream(msg.body));
        } catch {
          /* 無視 */
        }
      };
      ws.onclose = () => {
        if (!closed) retry = window.setTimeout(connect, 3000);
      };
      ws.onerror = () => ws?.close();
    }

    connect();
    return () => {
      closed = true;
      if (retry) window.clearTimeout(retry);
      ws?.close();
    };
  }, []);
}
