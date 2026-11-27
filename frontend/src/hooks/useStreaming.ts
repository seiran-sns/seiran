import { useCallback, useEffect, useRef } from "react";
import { getToken } from "../api/client";

/**
 * リアルタイム更新の WebSocket 接続（#37）。
 * 受信イベントを `onEvent(type, body)` に渡す。切断時は自動再接続する。
 * `onOpen` は接続確立（再接続含む）のたびに呼ばれる（チャンネル購読の再送に使う）。引数の
 * `resync` は、接続していない間があった（切断からの再接続、または非表示のまま開かれて表示時に
 * 初めてつないだ）とき true。その間に届くはずだったイベントは失われているので、呼び出し元は
 * REST で取り直す。ページを開いて即つないだ最初の接続では false（初期表示の取得と重複するため）。
 * 戻り値の `send` で任意のメッセージを送信できる（未接続時は何もしない）。
 *
 * ページが非表示（`document.visibilityState === "hidden"`、別タブ・最小化・スマホでアプリを
 * 離れたとき）の間は接続を切り、表示されたらつなぎ直す。見ていない画面へ配信し続ける帯域を
 * 省くため。表示時の取り直しで、離れていた間の新着が一度に届く。
 */
export function useStreaming(
  onEvent: (type: string, body: unknown) => void,
  reconnectKey?: unknown,
  onOpen?: (resync: boolean) => void
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
    // 次の接続確立時に取り直しを促すか（接続していない間があったか）
    let needResync = false;

    function connect() {
      retry = null;
      if (closed || wsRef.current || document.visibilityState === "hidden") return;
      const proto = location.protocol === "https:" ? "wss" : "ws";
      const ws = new WebSocket(`${proto}://${location.host}/api/streaming?token=${encodeURIComponent(token!)}`);
      wsRef.current = ws;
      ws.onopen = () => {
        const resync = needResync;
        needResync = false;
        onOpenRef.current?.(resync);
      };
      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          if (msg && typeof msg.type === "string") onEventRef.current(msg.type, msg.body);
        } catch {
          /* 無視 */
        }
      };
      ws.onclose = () => {
        // 非表示で自分から切った接続は wsRef から外し済み。ここで再接続を予約すると
        // 非表示のまま張り直してしまう
        if (wsRef.current !== ws) return;
        wsRef.current = null;
        needResync = true;
        if (!closed) retry = window.setTimeout(connect, 3000);
      };
      ws.onerror = () => ws.close();
    }

    function disconnect() {
      if (retry) window.clearTimeout(retry);
      retry = null;
      const ws = wsRef.current;
      wsRef.current = null;
      ws?.close();
    }

    function onVisibilityChange() {
      if (document.visibilityState === "hidden") {
        needResync = true;
        disconnect();
      } else {
        connect();
      }
    }

    // 非表示のまま開かれた（バックグラウンドのタブで開いた等）なら表示時につなぐ。それまでの
    // 新着は初期表示の取得に含まれないので、つないだときに取り直す
    if (document.visibilityState === "hidden") needResync = true;
    connect();
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      closed = true;
      document.removeEventListener("visibilitychange", onVisibilityChange);
      disconnect();
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
