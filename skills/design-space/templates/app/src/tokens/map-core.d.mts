import type { Kind, Token } from "./parse.mjs";
export const KIND_ORDER: readonly Kind[];
export function groupByKind(tokens: Token[]): Array<[Kind, Token[]]>;
export function mergeTokens(...lists: Token[][]): Token[];
export function mergeLibrary(product: Token[], library: Token[]): Token[];
export function liveValueOf(token: Token, style: Pick<CSSStyleDeclaration, "getPropertyValue">): string;
export function resolveVars(
  value: string,
  byVar: Map<string, Token>,
  style: Pick<CSSStyleDeclaration, "getPropertyValue">,
  seen?: Set<string>,
): string;
export function resolvedValueOf(
  token: Token,
  byVar: Map<string, Token>,
  style: Pick<CSSStyleDeclaration, "getPropertyValue">,
): string;
export function summaryLine(tokens: Token[]): string;
export function adoptAliasKinds(tokens: Token[]): Token[];
