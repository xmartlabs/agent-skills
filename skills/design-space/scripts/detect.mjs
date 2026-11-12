// Detects how a consuming repository styles itself. Pure reads, Node built-ins only.
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const ALWAYS_SKIPPED = new Set(['node_modules', '.git', 'dist']);
// Build output and static assets: never product stylesheets the design space should import.
const OUTPUT_DIRS = new Set(['build', 'out', 'coverage', 'public']);
const COMPONENT_DIRS = ['src/components/ui', 'components/ui', 'src/ui', 'ui'];
const TAILWIND_CONFIGS = ['js', 'cjs', 'mjs', 'ts'].map((e) => `tailwind.config.${e}`);

// A stylesheet that loads Tailwind: v4 `@import "tailwindcss"` (any quotes) or v3 `@tailwind <layer>`.
export const IMPORTS_TAILWIND = /@import\s+["']tailwindcss["']/;
const LOADS_TAILWIND = /@import\s+["']tailwindcss["']|@tailwind\s+(base|components|utilities|variants|screens)\b/;

const readText = (p) => { try { return readFileSync(p, 'utf8'); } catch { return null; } };
const isDir = (p) => { try { return statSync(p).isDirectory(); } catch { return false; } };

function tailwindMajor(root) {
  const raw = readText(join(root, 'package.json'));
  if (!raw) return null;
  let pkg;
  try { pkg = JSON.parse(raw); } catch { return null; }
  for (const group of ['dependencies', 'devDependencies', 'peerDependencies']) {
    const range = pkg[group]?.tailwindcss;
    const m = typeof range === 'string' && range.match(/\d+/);
    if (m) return Number(m[0]);
  }
  return null;
}

function listCss(root, skip) {
  const out = [];
  const walk = (rel) => {
    for (const e of readdirSync(join(root, rel), { withFileTypes: true })) {
      const r = rel ? `${rel}/${e.name}` : e.name;
      if (e.isDirectory()) {
        if (!ALWAYS_SKIPPED.has(e.name) && !skip.includes(r)) walk(r);
      } else if (e.name.endsWith('.css')) out.push(r);
    }
  };
  // Every top-level folder (src/, app/, styles/, ...), so a Next.js layout without src/ is found.
  for (const e of readdirSync(root, { withFileTypes: true })) {
    if (e.isFile() && e.name.endsWith('.css')) out.push(e.name);
    else if (e.isDirectory() && !e.name.startsWith('.') && !ALWAYS_SKIPPED.has(e.name)
      && !OUTPUT_DIRS.has(e.name) && !skip.includes(e.name)) walk(e.name);
  }
  return out;
}

// Tailwind v3 `darkMode: "class"` (or "selector", or ["class", ...]) toggles a `.dark` class.
const CLASS_DARK_MODE = /darkMode\s*:\s*\[?\s*["'](class|selector)["']/;

function themeMechanism(css, tailwindConfigText) {
  if (/(^|[\s,{}(])\.dark(?![\w-])/m.test(css)) return { kind: 'class', className: 'dark' };
  if (tailwindConfigText && CLASS_DARK_MODE.test(tailwindConfigText)) return { kind: 'class', className: 'dark' };
  const attr = css.match(/\[\s*(data-[\w-]+)\s*=\s*["']?dark["']?\s*\]/);
  if (attr) return { kind: 'attribute', attribute: attr[1], dark: 'dark', light: 'light' };
  if (/prefers-color-scheme\s*:\s*dark/.test(css)) return { kind: 'media' };
  return { kind: 'none' };
}

const FONT_IMPORT = /import\s*\{([^}]*)\}\s*from\s*["']next\/font\/google["']/g;
const LAYOUT_FILE = /^(layout|_app)\.[cm]?[jt]sx?$/;

// Google font families a Next.js app imports in its layouts (`Playfair_Display` -> "Playfair Display").
function nextFonts(root, skip) {
  const found = [];
  const visit = (rel, depth) => {
    let entries;
    try { entries = readdirSync(join(root, rel), { withFileTypes: true }); } catch { return; }
    for (const e of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const r = `${rel}/${e.name}`;
      if (e.isDirectory()) {
        if (depth < 6 && !ALWAYS_SKIPPED.has(e.name) && !skip.includes(r)) visit(r, depth + 1);
      } else if (LAYOUT_FILE.test(e.name) && (rel.split('/').pop() === 'pages' ? e.name.startsWith('_app') : e.name.startsWith('layout'))) {
        const text = readText(join(root, r)) ?? '';
        for (const m of text.matchAll(FONT_IMPORT)) {
          for (const part of m[1].split(',')) {
            const name = part.trim().split(/\s+as\s+/)[0].replaceAll('_', ' ');
            if (name && !found.includes(name)) found.push(name);
          }
        }
      }
    }
  };
  for (const dir of ['app', 'src/app', 'pages']) visit(dir, 0);
  return found;
}

/**
 * @param {string} root repository root
 * @param {{ components?: string, skip?: string[] }} [opts] `skip`: repository-relative folders
 *   never scanned or listed (default: the design space, `design-space`).
 */
export function detectProduct(root, opts = {}) {
  const skip = (opts.skip ?? ['design-space']).map((s) => s.replace(/\/+$/, ''));
  const underSkipped = (rel) => skip.some((s) => rel === s || rel.startsWith(`${s}/`));

  const stylesheets = [];
  const sources = [];
  for (const rel of listCss(root, skip)) {
    const css = readText(join(root, rel)) ?? '';
    // Token sources (custom properties, @theme) and Tailwind entry stylesheets; tokens come only from the first.
    if (/(^|[\s;{])--[\w-]+\s*:/.test(css) || /@theme\b/.test(css) || LOADS_TAILWIND.test(css)) {
      stylesheets.push(rel);
      sources.push(css);
    }
  }
  stylesheets.sort();

  const candidates = new Set(COMPONENT_DIRS.filter((d) => !underSkipped(d) && isDir(join(root, d))));
  const given = opts.components?.replace(/\/+$/, '');
  if (given && !underSkipped(given) && isDir(join(root, given))) candidates.add(given);

  const tailwindConfig = TAILWIND_CONFIGS.find((f) => existsSync(join(root, f))) ?? null;
  return {
    tailwind: tailwindMajor(root),
    stylesheets,
    tailwindConfig,
    theme: themeMechanism(sources.join('\n'), tailwindConfig && readText(join(root, tailwindConfig))),
    componentsCandidates: [...candidates].sort(),
    nextFonts: nextFonts(root, skip),
  };
}
