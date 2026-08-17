import { useCallback, useEffect, useRef } from "react";
import { getToken } from "../api/client";

/**
 * リアルタイム更新の WebSocket 接続（#37）。
 * 受信イベントを `onEvent(type, body)` に渡す。切断時は自動再接続する。
 * `onOpen` は接続確立（再接続含む）のたびに呼ばれる（チャンネル購読の再送に使う）。
 * 戻り値の `send` で任意のメッセージを送信できる（未接続時は何もしない）。
 */
export function useStreaming(
  onEvent: (type: string, body: unknown) => void,
  reconnectKey?: unknown,
  onOpen?: () => void
): { send: (msg: unknown) => void } {
  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;
  const onOpenRef = useRef(onOpen);
  onOpenRef.current = onOpen;
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    const token = getToken();
    if (!token) return;

    let closed = false;
    let retry: number | null = null;

    function connect() {
      if (closed) return;
      const proto = location.protocol === "https:" ? "wss" : "ws";
      const ws = new WebSocket(`${proto}://${location.host}/api/streaming?token=${encodeURIComponent(token!)}`);
      wsRef.current = ws;
      ws.onopen = () => onOpenRef.current?.();
      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          if (msg && typeof msg.type === "string") onEventRef.current(msg.type, msg.body);
        } catch {
          /* 無視 */
        }
      };
      ws.onclose = () => {
        if (wsRef.current === ws) wsRef.current = null;
        if (!closed) retry = window.setTimeout(connect, 3000);
      };
      ws.onerror = () => ws.close();
    }

    connect();
    return () => {
      closed = true;
      if (retry) window.clearTimeout(retry);
      wsRef.current?.close();
      wsRef.current = null;
    };
    // reconnectKey（ログイン状態など）が変わったら張り直す
  }, [reconnectKey]);

  const send = useCallback((msg: unknown) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }, []);

  return { send };
}
