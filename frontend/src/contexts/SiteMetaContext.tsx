import { createContext, useContext, useEffect, useState } from "react";
import { api } from "../api/client";

/**
 * サイト外観メタ（名称・カラー・アイコン）を全体へ供給する（issue #30）。
 * `POST /api/meta` を起動時に取得し、`site_color` を CSS 変数へ反映する。
 */
interface SiteMeta {
  name: string;
  iconUrl: string;
  color: string;
}

const SiteMetaContext = createContext<SiteMeta>({ name: "seiran", iconUrl: "", color: "" });

/** site_color から派生アクセント色を CSS 変数に適用する。 */
function applyColor(color: string) {
  const root = document.documentElement.style;
  if (!color) {
    // 既定に戻す（インラインで上書きした分をクリア）
    ["--accent", "--accent-strong", "--accent-hover", "--accent-deep", "--accent-deep-hover"].forEach((v) =>
      root.removeProperty(v)
    );
    return;
  }
  root.setProperty("--accent", color);
  root.setProperty("--accent-strong", color);
  root.setProperty("--accent-hover", `color-mix(in srgb, ${color} 82%, black)`);
  root.setProperty("--accent-deep", `color-mix(in srgb, ${color} 14%, white)`);
  root.setProperty("--accent-deep-hover", `color-mix(in srgb, ${color} 24%, white)`);
}

export function SiteMetaProvider({ children }: { children: React.ReactNode }) {
  const [meta, setMeta] = useState<SiteMeta>({ name: "seiran", iconUrl: "", color: "" });

  useEffect(() => {
    const controller = new AbortController();
    api
      .meta(controller.signal)
      .then((m) => {
        const next = { name: m.name || "seiran", iconUrl: m.siteIconUrl ?? "", color: m.siteColor ?? "" };
        setMeta(next);
        applyColor(next.color);
        if (next.name) document.title = next.name;
      })
      .catch(() => {});
    return () => controller.abort();
  }, []);

  return <SiteMetaContext.Provider value={meta}>{children}</SiteMetaContext.Provider>;
}

export function useSiteMeta() {
  return useContext(SiteMetaContext);
}
