import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";

/** 表示テーマの選択肢（#127）。「system」は端末の prefers-color-scheme に追従する。 */
export type ThemePreference = "system" | "light" | "dark";
type EffectiveTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "seiran_theme";

function isThemePreference(value: string | null): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

function systemPrefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function resolveEffectiveTheme(preference: ThemePreference): EffectiveTheme {
  return preference === "system" ? (systemPrefersDark() ? "dark" : "light") : preference;
}

function applyTheme(effective: EffectiveTheme) {
  document.documentElement.setAttribute("data-theme", effective);
}

interface ThemeContextValue {
  preference: ThemePreference;
  effectiveTheme: EffectiveTheme;
  setPreference: (preference: ThemePreference) => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  preference: "system",
  effectiveTheme: "light",
  setPreference: () => {},
});

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [preference, setPreferenceState] = useState<ThemePreference>(() => {
    const stored = localStorage.getItem(THEME_STORAGE_KEY);
    return isThemePreference(stored) ? stored : "system";
  });
  const [effectiveTheme, setEffectiveTheme] = useState<EffectiveTheme>(() => resolveEffectiveTheme(preference));

  useEffect(() => {
    const effective = resolveEffectiveTheme(preference);
    setEffectiveTheme(effective);
    applyTheme(effective);

    if (preference !== "system") return;
    // 「環境に従う」選択時は OS/ブラウザのテーマ変更をリアルタイムに反映する。
    const mql = window.matchMedia("(prefers-color-scheme: dark)");
    const handleChange = () => {
      const next = systemPrefersDark() ? "dark" : "light";
      setEffectiveTheme(next);
      applyTheme(next);
    };
    mql.addEventListener("change", handleChange);
    return () => mql.removeEventListener("change", handleChange);
  }, [preference]);

  const setPreference = useCallback((next: ThemePreference) => {
    setPreferenceState(next);
    localStorage.setItem(THEME_STORAGE_KEY, next);
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ preference, effectiveTheme, setPreference }),
    [preference, effectiveTheme, setPreference]
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme() {
  return useContext(ThemeContext);
}
