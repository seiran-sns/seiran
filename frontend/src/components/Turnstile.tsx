import { useCallback, useEffect, useRef, useState } from "react";

declare global {
  interface Window {
    turnstile?: {
      render: (element: HTMLElement, options: Record<string, unknown>) => string;
      reset: (widgetId: string) => void;
      remove: (widgetId: string) => void;
    };
  }
}

interface Props {
  siteKey: string;
  onToken: (token: string) => void;
  /** 値が変わるたびにウィジェットをリセットし、新しいトークンを取り直す。 */
  resetKey?: number;
}

export default function Turnstile({ siteKey, onToken, resetKey = 0 }: Props) {
  const container = useRef<HTMLDivElement>(null);
  const widgetIdRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (!siteKey || !container.current) return;
    let cancelled = false;
    const render = () => {
      if (cancelled || widgetIdRef.current || !container.current || !window.turnstile) return;
      widgetIdRef.current = window.turnstile.render(container.current, {
        sitekey: siteKey,
        callback: onToken,
        "expired-callback": () => onToken(""),
        "error-callback": () => onToken(""),
      });
    };
    let script = document.querySelector<HTMLScriptElement>('script[data-seiran-turnstile]');
    if (!script) {
      script = document.createElement("script");
      script.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
      script.async = true;
      script.defer = true;
      script.dataset.seiranTurnstile = "true";
      document.head.appendChild(script);
    }
    script.addEventListener("load", render);
    render();
    return () => {
      cancelled = true;
      script?.removeEventListener("load", render);
      if (widgetIdRef.current) window.turnstile?.remove(widgetIdRef.current);
      widgetIdRef.current = undefined;
    };
  }, [siteKey, onToken]);

  useEffect(() => {
    if (resetKey === 0 || !widgetIdRef.current) return;
    window.turnstile?.reset(widgetIdRef.current);
  }, [resetKey]);

  return siteKey ? <div ref={container} /> : null;
}

/**
 * Turnstile トークンの保持とリセット。トークンは Siteverify を1回通ると使用済みになり、
 * 同じトークンを再送すると必ず `TURNSTILE_FAILED` になる。送信が失敗したら（サーバー側で
 * Siteverify 済みの可能性があるので）`reset` でウィジェットを解き直させる。
 */
export function useTurnstileToken() {
  const [token, setToken] = useState("");
  const [resetKey, setResetKey] = useState(0);
  const reset = useCallback(() => {
    setToken("");
    setResetKey((k) => k + 1);
  }, []);
  return { token, onToken: setToken, resetKey, reset };
}
