import { readColor, readColorValue } from "./color.mjs";
import { cssColorOf } from "../tokens/parse.mjs";
import { liveValue, tokensByVar, type Token } from "../tokens/map";

type RGB = { r: number; g: number; b: number };

/**
 * Reads the color of a token, named by CSS variable or (theme-object tokens) by name, for the
 * active theme. A variable the page emits is read live; one it does not emit falls back to the
 * token's declared value, with var() chains resolved. Null when neither is a color.
 */
export function tokenColorReader(tokens: Token[]): (key: string) => RGB | null {
  const byVar = tokensByVar(tokens);
  const byName = new Map<string, Token>();
  for (const token of tokens) if (!token.cssVar && !byName.has(token.name)) byName.set(token.name, token);
  return (key) => {
    const named = byName.get(key);
    if (named) return readColorValue(cssColorOf(named.value));
    if (!key.startsWith("--")) return null;
    const live = readColor(key);
    const token = byVar.get(key);
    if (live || !token) return live;
    return readColorValue(cssColorOf(liveValue(token, byVar)));
  };
}
