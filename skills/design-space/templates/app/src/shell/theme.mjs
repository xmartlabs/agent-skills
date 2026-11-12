// Pure light/dark helpers. The mechanism comes from src/theme.ts (detected at scaffold time).
export const THEME_KEY = "design-space:theme";
export const THEME_EVENT = "design-space:theme-change";

export const canSwitch = (mechanism) => mechanism.kind === "class" || mechanism.kind === "attribute";

export function applyTheme(root, mechanism, theme) {
  if (mechanism.kind === "class") {
    if (theme === "dark") root.classList.add(mechanism.className);
    else root.classList.remove(mechanism.className);
  } else if (mechanism.kind === "attribute") {
    root.setAttribute(mechanism.attribute, theme === "dark" ? mechanism.dark : mechanism.light);
  }
}

// The default theme is light. A product with one theme never reads or writes storage.
export function storedTheme(storage, mechanism) {
  if (!canSwitch(mechanism)) return "light";
  try {
    return storage.getItem(THEME_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

export function saveTheme(storage, mechanism, theme) {
  if (!canSwitch(mechanism)) return;
  try {
    storage.setItem(THEME_KEY, theme);
  } catch {
    // Storage can be blocked; the theme still applies for this page.
  }
}

// Every window (top and preview frames) follows the theme another window saved.
export function syncThemeAcrossFrames(win, mechanism) {
  if (!canSwitch(mechanism)) return;
  win.addEventListener("storage", (event) => {
    if (event.key !== THEME_KEY) return;
    applyTheme(win.document.documentElement, mechanism, storedTheme(win.localStorage, mechanism));
    win.dispatchEvent(new Event(THEME_EVENT));
  });
}
