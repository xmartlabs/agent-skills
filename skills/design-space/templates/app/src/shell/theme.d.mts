export type Theme = "light" | "dark";
export type ThemeMechanism =
  | { readonly kind: "class"; readonly className: string }
  | { readonly kind: "attribute"; readonly attribute: string; readonly dark: string; readonly light: string }
  | { readonly kind: "media" }
  | { readonly kind: "none" };
export const THEME_KEY: string;
export const THEME_EVENT: string;
export function canSwitch(mechanism: ThemeMechanism): boolean;
export function applyTheme(
  root: {
    classList: { add(name: string): void; remove(name: string): void };
    setAttribute(name: string, value: string): void;
    removeAttribute(name: string): void;
  },
  mechanism: ThemeMechanism,
  theme: Theme,
): void;
export function storedTheme(storage: Pick<Storage, "getItem">, mechanism: ThemeMechanism): Theme;
export function saveTheme(storage: Pick<Storage, "setItem">, mechanism: ThemeMechanism, theme: Theme): void;
export function syncThemeAcrossFrames(
  win: Pick<Window, "addEventListener" | "dispatchEvent" | "localStorage" | "document">,
  mechanism: ThemeMechanism,
): void;
