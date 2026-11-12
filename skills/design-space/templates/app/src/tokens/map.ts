import { productCss, productThemes } from "../product-tokens";
import { adoptAliasKinds, mergeLibrary, mergeTokens, resolvedValueOf } from "./map-core.mjs";
import { flattenTheme, parseCssTokens, type Token } from "./parse.mjs";

// Design space tokens: the CSS custom properties declared in library/**/*.css.
const libraryStyles = import.meta.glob<string>("../../library/**/*.css", { query: "?raw", import: "default", eager: true });

let cached: Token[] | null = null;

/**
 * Every token: product CSS, product theme object, then the library; product wins on duplicates.
 * A library token that redefines a product variable stays in the map, flagged `overrides`, so the
 * audits can report it. An alias whose name says nothing takes the kind of the token it points to.
 */
export function tokenMap(): Token[] {
  cached ??= adoptAliasKinds(
    mergeLibrary(
      mergeTokens(
        productCss.flatMap((css) => parseCssTokens(css, "product")),
        productThemes.flatMap((theme) => flattenTheme(theme)),
      ),
      Object.values(libraryStyles).flatMap((css) => parseCssTokens(css, "library")),
    ),
  );
  return cached;
}

/** CSS variable -> its token; the first token (the product's) wins. */
export function tokensByVar(tokens: Token[] = tokenMap()): Map<string, Token> {
  const byVar = new Map<string, Token>();
  for (const token of tokens) if (token.cssVar && !byVar.has(token.cssVar)) byVar.set(token.cssVar, token);
  return byVar;
}

const rootStyle = () => getComputedStyle(document.documentElement);

/** Computed value of the token's variable for the active theme; empty when the page does not emit it. */
export function emittedValue(token: Token): string {
  return token.cssVar ? rootStyle().getPropertyValue(token.cssVar).trim() : token.value;
}

/**
 * Value for the active theme: the computed variable, or, when the page does not emit it (Tailwind
 * v4 `@theme` emits only what a class uses, `@theme inline` nothing), the declared value with
 * var() chains resolved.
 */
export function liveValue(token: Token, byVar: Map<string, Token> = tokensByVar()): string {
  return resolvedValueOf(token, byVar, rootStyle());
}

export { groupByKind } from "./map-core.mjs";
export type { Token };
