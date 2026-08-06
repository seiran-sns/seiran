import { useEffect, useRef } from "react";

declare global {
  interface Window {
    turnstile?: {
      render: (element: HTMLElement, options: Record<string, unknown>) => string;
      remove: (widgetId: string) => void;
    };
  }
}

interface Props {
  siteKey: string;
  onToken: (token: string) => void;
}

export default function Turnstile({ siteKey, onToken }: Props) {
  const container = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!siteKey || !container.current) return;
    let widgetId: string | undefined;
    let cancelled = false;
    const render = () => {
      if (cancelled || widgetId || !container.current || !window.turnstile) return;
      widgetId = window.turnstile.render(container.current, {
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
      if (widgetId) window.turnstile?.remove(widgetId);
    };
  }, [siteKey, onToken]);

  return siteKey ? <div ref={container} /> : null;
}
