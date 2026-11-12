import { useCallback, useEffect, useState } from "react";
import mechanism from "../theme";
import { MoonIcon, SunIcon } from "./icons";
import { THEME_EVENT, applyTheme, canSwitch, saveTheme, storedTheme, syncThemeAcrossFrames, type Theme } from "./theme.mjs";

// Apply the stored theme before the first render (no flash); preview frames call it too.
export function applyStoredTheme() {
  applyTheme(document.documentElement, mechanism, storedTheme(window.localStorage, mechanism));
}

export const syncTheme = () => syncThemeAcrossFrames(window, mechanism);

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(() => storedTheme(window.localStorage, mechanism));

  // The theme also changes through storage events (see syncThemeAcrossFrames).
  useEffect(() => {
    const onChange = () => setThemeState(storedTheme(window.localStorage, mechanism));
    window.addEventListener(THEME_EVENT, onChange);
    return () => window.removeEventListener(THEME_EVENT, onChange);
  }, []);

  const setTheme = useCallback((next: Theme) => {
    saveTheme(window.localStorage, mechanism, next);
    applyTheme(document.documentElement, mechanism, next);
    window.dispatchEvent(new Event(THEME_EVENT));
  }, []);

  return { theme, setTheme, canSwitch: canSwitch(mechanism) };
}

export function ThemeSwitch() {
  const { theme, setTheme, canSwitch: switchable } = useTheme();
  if (!switchable) return null;
  const next = theme === "dark" ? "light" : "dark";
  // Two segments, the active theme pressed; the button's name says what a click does.
  return (
    <button type="button" className="ds-theme-switch" aria-label={`Switch to ${next} theme`} onClick={() => setTheme(next)}>
      <span className="ds-theme-option" data-active={theme === "light"}><SunIcon size={14} /></span>
      <span className="ds-theme-option" data-active={theme === "dark"}><MoonIcon size={14} /></span>
    </button>
  );
}
