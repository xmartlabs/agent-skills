// Maps the shell's variables to the product's own tokens. Pure: also usable from the scaffold.
import { cssColorOf, isColorValue } from '../tokens/parse.mjs';

export const SHELL_VARS = [
  '--ds-bg', '--ds-fg', '--ds-muted', '--ds-muted-fg', '--ds-border', '--ds-primary',
  '--ds-primary-fg', '--ds-card', '--ds-radius', '--ds-font', '--ds-font-heading', '--ds-font-mono',
];

const SYSTEM_SANS = 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif';
const SYSTEM_MONO = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';

// Clean, neutral look used for every variable the product does not declare.
export const NEUTRAL = {
  light: {
    '--ds-bg': '#ffffff',
    '--ds-fg': '#0a0a0a',
    '--ds-muted': '#f4f4f5',
    '--ds-muted-fg': '#71717a',
    '--ds-border': '#e4e4e7',
    '--ds-primary': '#2563eb',
    '--ds-primary-fg': '#ffffff',
    '--ds-card': '#ffffff',
    '--ds-radius': '0.5rem',
    '--ds-font': SYSTEM_SANS,
    '--ds-font-heading': SYSTEM_SANS,
    '--ds-font-mono': SYSTEM_MONO,
  },
  // Only what differs in the dark theme (radius and fonts do not change).
  dark: {
    '--ds-bg': '#0a0a0a',
    '--ds-fg': '#fafafa',
    '--ds-muted': '#27272a',
    '--ds-muted-fg': '#a1a1aa',
    '--ds-border': '#27272a',
    '--ds-primary': '#3b82f6',
    '--ds-primary-fg': '#ffffff',
    '--ds-card': '#18181b',
  },
};

// Product custom properties to read for each shell variable, in order of preference.
export const CANDIDATES = {
  '--ds-bg': ['--background', '--color-background', '--bg', '--surface'],
  '--ds-fg': ['--foreground', '--color-foreground', '--fg', '--text', '--color-text'],
  '--ds-muted': ['--muted', '--color-muted', '--secondary', '--color-secondary'],
  '--ds-muted-fg': ['--muted-foreground', '--color-muted-foreground'],
  '--ds-border': ['--border', '--color-border'],
  '--ds-primary': ['--primary', '--color-primary', '--brand', '--color-brand'],
  '--ds-primary-fg': ['--primary-foreground', '--color-primary-foreground'],
  '--ds-card': ['--card', '--color-card'],
  '--ds-radius': ['--radius', '--radius-md', '--radius-lg'],
  '--ds-font': ['--font-sans', '--font-body', '--default-font-family'],
  '--ds-font-heading': ['--font-heading', '--font-display', '--font-serif'],
  '--ds-font-mono': ['--font-mono'],
};

function darkSelector(mechanism) {
  if (mechanism?.kind === 'class') return `.${mechanism.className}`;
  if (mechanism?.kind === 'attribute') return `[${mechanism.attribute}="${mechanism.dark}"]`;
  return null;
}

const NON_COLOR = new Set(['--ds-radius', '--ds-font', '--ds-font-heading', '--ds-font-mono']);
const SERIFISH = /serif|display|playfair|lora|merriweather|fraunces|cormorant|spectral/i;
const quote = (family) => `"${family}"`;

// A color shell variable only reads a color (or a reference to one); a font or radius reads anything.
const usable = (name, value) => NON_COLOR.has(name) || isColorValue(value) || /^var\(/.test(String(value).trim());

/**
 * @param {{ cssVar: string | null, value: string }[]} tokens the product's parsed tokens
 * @param {{ kind: string, className?: string, attribute?: string, dark?: string }} mechanism
 * @param {{ fonts?: string[] }} [options] `fonts`: Google families the design space loads. next/font
 *   defines its `--font-*` variables on elements below `:root`, so they are named as fallbacks.
 * @returns {{ css: string, mapped: Record<string, string | null> }}
 */
export function buildThemeMap(tokens, mechanism, options = {}) {
  const fonts = options.fonts ?? [];
  const sans = fonts.find((f) => !SERIFISH.test(f) && !/mono/i.test(f)) ?? fonts[0];
  const heading = fonts.find((f) => SERIFISH.test(f)) ?? fonts[0];
  // What follows the product variable (or stands alone when it has none).
  const fontFallback = {
    '--ds-font': [...(sans ? [quote(sans)] : []), NEUTRAL.light['--ds-font']],
    '--ds-font-heading': [...(heading ? [quote(heading)] : []), 'var(--ds-font)'],
  };
  const byVar = new Map(tokens.filter((t) => t.cssVar).map((t) => [t.cssVar, t]));
  const mapped = {};
  const light = [];
  const fellBack = [];
  for (const name of SHELL_VARS) {
    const found = CANDIDATES[name].map((c) => byVar.get(c)).find((t) => t && usable(name, t.value));
    mapped[name] = found ? found.cssVar : null;
    const fallback = fontFallback[name]?.join(', ') ?? NEUTRAL.light[name];
    if (!found) {
      if (!(name in fontFallback)) fellBack.push(name);
      light.push(`  ${name}: ${fontFallback[name] ? fallback : NEUTRAL.light[name]};`);
      continue;
    }
    const value = String(found.value).trim();
    // Bare HSL channels (shadcn style) only paint inside hsl().
    light.push(cssColorOf(value) !== value
      ? `  ${name}: hsl(var(${found.cssVar}, ${value}));`
      : `  ${name}: var(${found.cssVar}, ${fallback});`);
  }
  const lines = [
    '/* Maps the design space shell to the product\'s tokens. Generated at scaffold; engineer-owned. */',
    ':root {', ...light, '}',
  ];
  const selector = darkSelector(mechanism);
  const dark = fellBack.filter((name) => name in NEUTRAL.dark).map((name) => `  ${name}: ${NEUTRAL.dark[name]};`);
  if (selector && dark.length) lines.push(`${selector} {`, ...dark, '}');
  // The OS setting picks the theme: the same neutral dark values, nested under the media query.
  if (mechanism?.kind === 'media' && dark.length) {
    lines.push('@media (prefers-color-scheme: dark) {', '  :root {', ...dark.map((line) => `  ${line}`), '  }', '}');
  }
  return { css: `${lines.join('\n')}\n`, mapped };
}
