export type Kind = "color" | "spacing" | "font" | "text" | "radius" | "shadow" | "breakpoint" | "other";
export type Token = {
  name: string;
  kind: Kind;
  value: string;
  cssVar: string | null;
  source: "product" | "library";
  inline: boolean;
  /** A library token that redefines a product variable (see mergeLibrary). */
  overrides?: boolean;
};
export function kindOf(name: string, value?: string): Kind;
export function isColorValue(value: string): boolean;
export function cssColorOf(value: string): string;
export function parseCssTokens(css: string, source: "product" | "library"): Token[];
export function flattenTheme(theme: object): Token[];
