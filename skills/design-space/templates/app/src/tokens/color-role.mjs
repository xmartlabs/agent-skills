// Pure color grouping: no node: imports, it also runs in the browser bundle.

/** @typedef {"brand"|"status"|"surface"|"palette"|"other"} ColorRole */

/** @type {readonly ColorRole[]} */
export const ROLE_ORDER = ['brand', 'status', 'surface', 'palette', 'other'];

/** @type {Record<ColorRole, string>} */
export const ROLE_LABELS = {
  brand: 'Brand',
  status: 'Status',
  surface: 'Surface & text',
  palette: 'Palette',
  other: 'Other colors',
};

const PALETTE = /-(50|[1-9]00|950)$/;
const STATUS = /^(success|destructive|warning|error|danger|info)(-.+)?$/;
const BRAND = /^(primary|secondary|accent|brand)(-.+)?$/;
const SURFACE = /^(background|foreground|card|popover|muted|border|input|ring|text|surface|sidebar)(-.+)?$|-foreground$/;

/**
 * Role of a color token by its name (`--primary`, `color-gray-100`, Tailwind v3 `colors-brand-500`).
 * @param {string} name @returns {ColorRole}
 */
export function colorRole(name) {
  const n = name.replace(/^--/, '').replace(/^colors?-/, '');
  if (PALETTE.test(n)) return 'palette';
  if (STATUS.test(n)) return 'status';
  if (BRAND.test(n)) return 'brand';
  if (SURFACE.test(n)) return 'surface';
  return 'other';
}

/** `var(--violet-600)` -> `violet-600`; null for anything else. @param {string} value @returns {string | null} */
export function paletteRefOf(value) {
  const m = /^\s*var\(\s*--([\w-]+)\s*\)\s*$/.exec(String(value ?? ''));
  return m ? m[1] : null;
}

const SCALE = /^(.*)-(\d+)$/;
const byName = (a, b) => a.name.localeCompare(b.name);
// Palette: by family (`violet`, `slate`), then by scale number, so -50 comes before -100.
const byScale = (a, b) => {
  const [, fa, na] = SCALE.exec(a.name) ?? [, a.name, '0'];
  const [, fb, nb] = SCALE.exec(b.name) ?? [, b.name, '0'];
  return fa.localeCompare(fb) || Number(na) - Number(nb);
};

/**
 * Color tokens grouped by role in ROLE_ORDER; roles without tokens are omitted. Tokens are sorted
 * by name within a role, the palette by family and scale number.
 * @template {{ name: string }} T @param {T[]} tokens @returns {Array<[ColorRole, T[]]>}
 */
export function groupColors(tokens) {
  /** @type {Map<ColorRole, T[]>} */
  const groups = new Map(ROLE_ORDER.map((role) => [role, []]));
  for (const token of tokens) groups.get(colorRole(token.name))?.push(token);
  return [...groups]
    .filter(([, list]) => list.length > 0)
    .map(([role, list]) => [role, [...list].sort(role === 'palette' ? byScale : byName)]);
}

/** `{ r: 124, g: 58, b: 237 }` -> `#7C3AED`. @param {{ r: number, g: number, b: number }} rgb */
export function hexOf({ r, g, b }) {
  const hex = (v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0');
  return `#${hex(r)}${hex(g)}${hex(b)}`.toUpperCase();
}

/**
 * The token a color points to in the active theme. The declared reference (`var(--violet-600)`)
 * holds while it still has the color; a theme can point elsewhere (dark: `var(--violet-500)`),
 * and the source parser reads only the default theme, so a stale reference, or a literal that the
 * theme changed, is replaced by the palette token with the same hex. Null when there is none.
 * @param {{ ref: string | null, declaredHex: string | null, hex: string | null }} color
 * @param {Map<string, string>} hexByName token name -> hex in the active theme
 * @returns {string | null}
 */
export function themeRefOf({ ref, declaredHex, hex }, hexByName) {
  if (!hex) return ref;
  const stale = ref ? hexByName.has(ref) && hexByName.get(ref) !== hex : declaredHex !== hex;
  if (!stale) return ref;
  for (const [name, value] of hexByName) if (value === hex && colorRole(name) === 'palette') return name;
  return null;
}
