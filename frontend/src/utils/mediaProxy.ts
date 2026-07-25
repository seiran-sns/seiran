let externalProxyBase = "";

export function configureMediaProxy(base: string) {
  externalProxyBase = base.trim().replace(/\/+$/, "");
}

/** 同一オリジンと非HTTP URLはそのままにし、リモートメディアだけをプロキシへ通す。 */
export function mediaUrl(raw?: string | null): string | undefined {
  if (!raw) return undefined;
  try {
    const target = new URL(raw, window.location.origin);
    if (!["http:", "https:"].includes(target.protocol) || target.origin === window.location.origin) return raw;
    const proxy = externalProxyBase ? `${externalProxyBase}/proxy` : "/proxy";
    return `${proxy}?url=${encodeURIComponent(target.href)}`;
  } catch {
    return raw;
  }
}
