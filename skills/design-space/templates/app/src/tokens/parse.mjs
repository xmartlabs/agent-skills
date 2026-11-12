// Pure token parsing: no node: imports, it also runs in the browser bundle.

/** @typedef {"color"|"spacing"|"font"|"text"|"radius"|"shadow"|"breakpoint"|"other"} Kind */
/**
 * @typedef {{ name: string, kind: Kind, value: string, cssVar: string | null,
 *   source: "product" | "library", inline: boolean, overrides?: boolean }} Token
 */

// Bare shadcn names (no `color-` prefix) are colors.
const BARE_COLORS = /^(background|foreground|primary|secondary|card|popover|muted|accent|destructive|border|input|ring|chart-.+|sidebar(-.+)?)$/;

// Tailwind config theme keys, flattened names start with one of these.
const THEME_KEYS = [
  ['colors?', 'color'], ['spacing', 'spacing'], ['space', 'spacing'],
  ['fontFamily|font', 'font'], ['fontSize|text', 'text'],
  ['borderRadius|radius', 'radius'], ['boxShadow|shadow', 'shadow'],
  ['screens|breakpoint', 'breakpoint'],
].map(([keys, kind]) => [new RegExp(`^(${keys})(-|$)`), kind]);

const FN_COLOR = /^(rgba?|hsla?|oklch|oklab|lab|lch|color)\(.+\)$/i;
const HEX_COLOR = /^#([\da-f]{3,4}|[\da-f]{6}|[\da-f]{8})$/i;
// shadcn-style bare HSL channels: `222.2 84% 4.9%`, optionally `/ alpha`.
const BARE_HSL = /^-?[\d.]+(deg)?\s+[\d.]+%\s+[\d.]+%(\s*\/\s*[\d.]+%?)?$/;

/** Whether a declared value is a color: hex, a color function, or bare HSL channels. @param {string} value */
export function isColorValue(value) {
  const v = String(value ?? '').trim();
  return HEX_COLOR.test(v) || FN_COLOR.test(v) || BARE_HSL.test(v);
}

/** A value ready to paint: bare HSL channels become `hsl(...)`, anything else is unchanged. @param {string} value */
export function cssColorOf(value) {
  const v = String(value ?? '').trim();
  return BARE_HSL.test(v) ? `hsl(${v})` : value;
}

/**
 * @param {string} name custom property (`--x`) or flattened theme name (`colors-x`)
 * @param {string} [value] declared value: decides when the name does not say, and for `text-*`
 * @returns {Kind}
 */
export function kindOf(name, value) {
  const n = name.replace(/^--/, '');
  if (BARE_COLORS.test(n) || /-foreground$/.test(n)) return 'color';
  const byValue = value !== undefined && isColorValue(value);
  for (const [re, kind] of THEME_KEYS) if (re.test(n)) return kind === 'text' && byValue ? 'color' : kind;
  return byValue ? 'color' : 'other';
}

/** @param {string} css @param {"product"|"library"} source @returns {Token[]} */
export function parseCssTokens(css, source) {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
  /** @type {Map<string, Token>} */
  const found = new Map();

  const scan = (body, dark) => {
    let i = 0;
    while (i < body.length) {
      const open = body.indexOf('{', i);
      if (open === -1) break;
      // Brace-less statements (`@import ...;`, `@layer a, b;`) end at `;`: the prelude follows the last one.
      const prelude = body.slice(i, open).split(/[;}]/).pop().trim();
      let depth = 1;
      let j = open + 1;
      while (j < body.length && depth > 0) {
        if (body[j] === '{') depth++;
        else if (body[j] === '}') depth--;
        j++;
      }
      const inner = body.slice(open + 1, j - 1);
      i = j;
      if (/^@(media|layer|supports)\b/.test(prelude)) {
        scan(inner, dark || /prefers-color-scheme\s*:\s*dark/.test(prelude));
      } else if (/^@theme\b/.test(prelude)) {
        collect(inner, /\binline\b/.test(prelude));
      } else if (!dark && prelude.split(',').some((s) => /^(:root|html|:host)$/.test(s.trim()))) {
        collect(inner, false);
      }
    }
  };

  const collect = (inner, inline) => {
    for (const m of inner.matchAll(/(--[\w-]+)\s*:\s*([^;]+?)\s*(?:;|$)/g)) {
      const [, cssVar, value] = m;
      const name = cssVar.slice(2);
      if (name.endsWith('-*') || found.has(name)) continue;
      found.set(name, { name, kind: kindOf(name, value), value, cssVar, source, inline });
    }
  };

  scan(text, false);
  return [...found.values()];
}

const stringify = (v) => {
  if (Array.isArray(v)) {
    // Tailwind fontSize: [size, { lineHeight }]
    if (v.length && typeof v[0] === 'string' && v.slice(1).some((x) => x && typeof x === 'object')) return v[0];
    return v.map(String).join(', ');
  }
  return String(v);
};

/** @param {object} theme Tailwind `theme` object @returns {Token[]} */
export function flattenTheme(theme) {
  /** @type {Token[]} */
  const out = [];
  const walk = (node, path) => {
    if (node && typeof node === 'object' && !Array.isArray(node)) {
      for (const [k, v] of Object.entries(node)) walk(v, k === 'DEFAULT' ? path : [...path, k]);
      return;
    }
    if (!path.length) return;
    const name = path.join('-');
    const value = stringify(node);
    out.push({ name, kind: kindOf(name, value), value, cssVar: null, source: 'product', inline: false });
  };
  walk(theme, []);
  return out;
}
