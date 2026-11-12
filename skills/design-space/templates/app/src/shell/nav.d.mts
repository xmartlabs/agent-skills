import type { Kind } from "../tokens/parse.mjs";

export type NavItem = { href: string; label: string };
export type NavSection = {
  key: "tokens" | "components" | "shared" | "prototypes" | "audits";
  label: string;
  items: NavItem[];
};
export type TokenPageDef = { slug: string; label: string; kinds: Kind[] };

export const TOKEN_PAGES: readonly TokenPageDef[];
export function tokenPages(kinds: readonly Kind[]): TokenPageDef[];
export function labelOf(name: string): string;
export function buildNav(input: {
  tokenKinds: readonly Kind[];
  components: readonly string[];
  shared: readonly string[];
  flows: readonly { name: string; title: string }[];
  audits: readonly { name: string; title: string }[];
}): NavSection[];
export function filterNav(sections: NavSection[], query: string): NavSection[];
export function productName(meta?: { name?: string | null; packageName?: string | null }): string;
