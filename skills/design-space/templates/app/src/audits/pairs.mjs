// Pure helpers for the audit pages: which colors to compare, and token usage findings.
// No Node imports: scaffold (Node) and the browser bundle both load this file.

/** @typedef {{ foreground: string, background: string, label: string, large: boolean }} Pair */

const FOREGROUNDS = ['--foreground', '--color-foreground', '--color-text'];
const SURFACES = ['--background', '--color-background', '--card', '--color-card'];
const STATUS = /success|error|danger|destructive|warning|info|chart/;

const varOf = (token) => token.cssVar ?? token.name;
const short = (cssVar) => cssVar.replace(/^--(?:color-)?/, '');
const ALIAS = /^var\(\s*(--[\w-]+)\s*\)$/;

/**
 * Alias groups: a token whose value is exactly `var(--y)` of another token is an alias of it
 * (shadcn's `@theme inline { --color-x: var(--x) }`). `group(key)` names the group a token
 * belongs to; `preferred(key)` is the name to report: an inline token, which is never emitted,
 * maps along its chain to the first emitted (non-inline) token, any other keeps its name.
 */
function aliases(tokens) {
  const byVar = new Map();
  for (const token of tokens) if (token.cssVar && !byVar.has(token.cssVar)) byVar.set(token.cssVar, token);
  const target = (key) => {
    const match = ALIAS.exec(String(byVar.get(key)?.value ?? '').trim());
    return match && match[1] !== key && byVar.has(match[1]) ? match[1] : null;
  };
  const parent = new Map();
  const find = (key) => {
    let root = key;
    while (parent.has(root)) root = parent.get(root);
    return root;
  };
  for (const key of byVar.keys()) {
    const to = target(key);
    if (!to) continue;
    const [a, b] = [find(key), find(to)];
    if (a !== b) parent.set(a, b);
  }
  const preferred = (key) => {
    const seen = new Set();
    let current = key;
    while (byVar.get(current)?.inline) {
      seen.add(current);
      const next = target(current);
      if (!next || seen.has(next)) return key;
      current = next;
    }
    return current;
  };
  return { group: find, preferred };
}

/** Contrast pairs worth checking, derived from token names; one pair per alias group pair. */
export function proposeContrastPairs(tokens) {
  const vars = new Set(tokens.map((t) => t.cssVar).filter(Boolean));
  const { group, preferred } = aliases(tokens);
  const out = [];
  const seen = new Set();
  const add = (fg, bg) => {
    if (!vars.has(fg) || !vars.has(bg) || group(fg) === group(bg)) return;
    const key = `${group(fg)}|${group(bg)}`;
    if (seen.has(key)) return;
    seen.add(key);
    const [foreground, background] = [preferred(fg), preferred(bg)];
    out.push({ foreground, background, label: `${short(foreground)} on ${short(background)}`, large: false });
  };
  for (const name of vars) {
    const match = /^(--.+)-foreground$/.exec(name);
    if (match) add(name, match[1]);
  }
  for (const foreground of FOREGROUNDS) for (const surface of SURFACES) add(foreground, surface);
  // Muted text on the page background, in shadcn and in Tailwind v4 (`--color-*`) names.
  add('--muted-foreground', '--background');
  add('--color-muted-foreground', '--color-background');
  return out;
}

/**
 * Color pairs to simulate under color-vision deficiency: contrast pairs and every status/chart pair.
 * Library tokens that override a product variable are the same variable: they add no pair.
 */
export function colorVisionPairs(tokens, contrastPairs) {
  const { group, preferred } = aliases(tokens);
  const out = [];
  const seen = new Set();
  // A token and its alias are one color: never a pair, and each pair of groups appears once.
  const add = (a, b) => {
    const key = [group(a), group(b)].sort().join('|');
    if (group(a) === group(b) || seen.has(key)) return;
    seen.add(key);
    out.push({ a: preferred(a), b: preferred(b) });
  };
  for (const pair of contrastPairs) add(pair.foreground, pair.background);
  const status = tokens.filter((t) => !t.overrides && t.kind === 'color' && STATUS.test(t.name)).map(varOf);
  for (let i = 0; i < status.length; i++) for (let j = i + 1; j < status.length; j++) add(status[i], status[j]);
  return out;
}

/**
 * Tokens with no value, and same-kind tokens sharing one. `values` maps token name to its live
 * (computed) value, empty when the variable is not emitted or invalid; `resolved` to its declared
 * value with var() chains resolved. A token is unresolved when it has no value at all, or when it
 * is not inline, its declared value uses var(), it computes to nothing and the chain resolves
 * nowhere.
 */
export function tokenUsageFindings(tokens, values, resolved = new Map()) {
  const unresolved = [];
  const groups = new Map();
  // A designer token redefining a product variable is reported on its own (see mergeLibrary).
  const overrides = tokens.filter((t) => t.overrides).map((t) => t.name);
  for (const token of tokens.filter((t) => !t.overrides)) {
    const live = String(values.get(token.name) ?? '').trim();
    const fallback = String(resolved.get(token.name) ?? '').trim();
    const value = live || fallback;
    const broken = !live && !token.inline && String(token.value).includes('var(') && (!fallback || fallback.includes('var('));
    if (!value || broken) {
      unresolved.push(token.name);
      continue;
    }
    const key = `${token.kind}|${value.toLowerCase()}`;
    const group = groups.get(key) ?? { value, names: [] };
    group.names.push(token.name);
    groups.set(key, group);
  }
  return { unresolved, duplicates: [...groups.values()].filter((g) => g.names.length > 1), overrides };
}
