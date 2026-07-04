import { useEffect, useRef } from "react";
import { getToken } from "../api/client";

/**
 * リアルタイム更新の WebSocket 接続（#37）。
 * 受信イベントを `onEvent(type, body)` に渡す。切断時は自動再接続する。
 */
export function useStreaming(
  onEvent: (type: string, body: unknown) => void,
  reconnectKey?: unknown
) {
  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;

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
          if (msg && typeof msg.type === "string") onEventRef.current(msg.type, msg.body);
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
    // reconnectKey（ログイン状態など）が変わったら張り直す
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reconnectKey]);
}
