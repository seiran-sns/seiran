import { createContext, useContext, useEffect, useState } from "react";
import { api } from "../api/client";
import { configureInternalMediaOrigins, configureMediaProxy } from "../utils/mediaProxy";
import { useTheme } from "./ThemeContext";

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

/** site_color から派生アクセント色を CSS 変数に適用する。インラインstyle（documentElement）は
 * `:root[data-theme="dark"]`より詳細度が高くダークモードの既定パレットを上書きしてしまうため、
 * `--accent-deep`系は現在の実効テーマに応じて混合先を変える（ライトは白、ダークは
 * `--bg-elev-2`）。white固定のままだと、ダークモードでも常に明るい薄色になり、同じくダーク
 * モードの`--text`（明るい色）と輝度がほぼ同化して見えなくなる不具合があった（マイケル指摘）。
 * `--accent`自体もダークモードではsite_colorをそのまま使わず、白寄りに60%混合して明るくする
 * （この混合率は、既定ダークパレットの`--accent: #60a5fa`と輝度がほぼ一致するよう選んだもの。
 * ライトモード用に選ばれがちなsite_colorをそのまま使うと、ダークモードの暗い背景の上で
 * 文字色として沈んで見づらくなるため）。`--accent-strong`はプライマリボタン等の背景色として
 * 使われることが大半（文字色としての使用箇所は無い）のため、`--accent`と同じ値にしてしまうと
 * ダークモードでボタン背景まで一緒に明るくなってしまう（マイケル指摘）。`--accent-strong`は
 * site_colorそのまま・テーマ非依存の値を保つ。 */
function applyColor(color: string, isDark: boolean) {
  const root = document.documentElement.style;
  if (!color) {
    // 既定に戻す（インラインで上書きした分をクリア）
    ["--accent", "--accent-strong", "--accent-hover", "--accent-deep", "--accent-deep-hover"].forEach((v) =>
      root.removeProperty(v)
    );
    return;
  }
  const deepMixTarget = isDark ? "var(--bg-elev-2)" : "white";
  const accentColor = isDark ? `color-mix(in srgb, ${color} 60%, white)` : color;
  root.setProperty("--accent", accentColor);
  root.setProperty("--accent-strong", color);
  root.setProperty("--accent-hover", `color-mix(in srgb, ${color} 82%, black)`);
  root.setProperty("--accent-deep", `color-mix(in srgb, ${color} 14%, ${deepMixTarget})`);
  root.setProperty("--accent-deep-hover", `color-mix(in srgb, ${color} 24%, ${deepMixTarget})`);
}

/** サイトアイコン（#42）を favicon (<link rel="icon">) に反映する。空なら既定へ戻す。 */
function applyFavicon(iconUrl: string) {
  let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) {
    link = document.createElement("link");
    link.rel = "icon";
    document.head.appendChild(link);
  }
  link.href = iconUrl || "/favicon.ico";
}

export function SiteMetaProvider({ children }: { children: React.ReactNode }) {
  const { effectiveTheme } = useTheme();
  const [meta, setMeta] = useState<SiteMeta>({ name: "seiran", iconUrl: "", color: "" });

  useEffect(() => {
    const controller = new AbortController();
    api
      .meta(controller.signal)
      .then((m) => {
        configureMediaProxy(m.mediaProxyUrl ?? "");
        configureInternalMediaOrigins(m.internalMediaOrigins ?? []);
        const next = { name: m.name || "seiran", iconUrl: m.siteIconUrl ?? "", color: m.siteColor ?? "" };
        setMeta(next);
        applyFavicon(next.iconUrl);
        if (next.name) document.title = next.name;
      })
      .catch(() => {});
    return () => controller.abort();
  }, []);

  // テーマ切替（ライト⇄ダーク）時にも、その時点の実効テーマで再計算して適用し直す。
  useEffect(() => {
    applyColor(meta.color, effectiveTheme === "dark");
  }, [meta.color, effectiveTheme]);

  return <SiteMetaContext.Provider value={meta}>{children}</SiteMetaContext.Provider>;
}

export function useSiteMeta() {
  return useContext(SiteMetaContext);
}
