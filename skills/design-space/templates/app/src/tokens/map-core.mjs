// Pure token-map logic: no node: imports, it also runs in the browser bundle.

export const KIND_ORDER = ['color', 'spacing', 'text', 'font', 'radius', 'shadow', 'breakpoint', 'other'];

/** Group tokens by kind in the fixed display order. Kinds without tokens are omitted. */
export function groupByKind(tokens) {
  const groups = new Map(KIND_ORDER.map((kind) => [kind, []]));
  for (const token of tokens) (groups.get(token.kind) ?? groups.get('other')).push(token);
  return [...groups].filter(([, list]) => list.length > 0);
}

/**
 * Merge token lists in priority order. A token is a duplicate of an earlier one when it has the
 * same `cssVar` (non-null) or, when `cssVar` is null, the same `name`: the earlier one wins.
 */
export function mergeTokens(...lists) {
  const seen = new Set();
  const out = [];
  for (const token of lists.flat()) {
    const key = token.cssVar ?? `name:${token.name}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(token);
  }
  return out;
}

/**
 * Product tokens followed by the library's. A library token that redefines a product variable is
 * kept, flagged `overrides: true`, and never silently dropped: a designer can change what the
 * audits measure that way, so the pages report it. Other duplicates merge as in mergeTokens.
 */
export function mergeLibrary(product, library) {
  const keyOf = (token) => token.cssVar ?? `name:${token.name}`;
  const productVars = new Set(product.map((t) => t.cssVar).filter(Boolean));
  const out = mergeTokens(product);
  const seen = new Set(out.map(keyOf));
  const overridden = new Set();
  for (const token of library) {
    const key = keyOf(token);
    if (token.cssVar && productVars.has(token.cssVar)) {
      if (overridden.has(key)) continue;
      overridden.add(key);
      out.push({ ...token, overrides: true });
    } else if (!seen.has(key)) {
      seen.add(key);
      out.push(token);
    }
  }
  return out;
}

/** Computed value of a token for the active theme; falls back to the declared value. */
export function liveValueOf(token, style) {
  if (!token.cssVar) return token.value;
  return style.getPropertyValue(token.cssVar).trim() || token.value;
}

// Index of the comma that separates a var() name from its fallback, outside nested parentheses.
function fallbackComma(inner) {
  let depth = 0;
  for (let i = 0; i < inner.length; i++) {
    if (inner[i] === '(') depth++;
    else if (inner[i] === ')') depth--;
    else if (inner[i] === ',' && depth === 0) return i;
  }
  return -1;
}

/**
 * Replace each `var(--x[, fallback])` in `value`: the live value of `--x` when the page has one,
 * else the declared value of the token `--x` (from `byVar`, resolved the same way), else the
 * resolved fallback. What cannot be resolved stays as written. `seen` guards against cycles.
 */
export function resolveVars(value, byVar, style, seen = new Set()) {
  const text = String(value ?? '');
  const resolveOne = (name, fallback) => {
    const live = style.getPropertyValue(name).trim();
    if (live) return live;
    const token = byVar.get(name);
    if (token && !seen.has(name)) {
      const declared = resolveVars(token.value, byVar, style, new Set([...seen, name]));
      if (!declared.includes('var(')) return declared;
    }
    if (fallback !== null) {
      const resolved = resolveVars(fallback, byVar, style, seen);
      if (!resolved.includes('var(')) return resolved;
    }
    return null;
  };
  let out = '';
  let i = 0;
  while (i < text.length) {
    const at = text.indexOf('var(', i);
    if (at === -1) return out + text.slice(i);
    out += text.slice(i, at);
    let depth = 0;
    let end = at + 3;
    for (; end < text.length; end++) {
      if (text[end] === '(') depth++;
      else if (text[end] === ')' && --depth === 0) break;
    }
    if (end >= text.length) return out + text.slice(at);
    const inner = text.slice(at + 4, end);
    const comma = fallbackComma(inner);
    const name = (comma === -1 ? inner : inner.slice(0, comma)).trim();
    const fallback = comma === -1 ? null : inner.slice(comma + 1).trim();
    out += resolveOne(name, fallback) ?? text.slice(at, end + 1);
    i = end + 1;
  }
  return out;
}

/**
 * Value of a token for the active theme. Its variable when the page emits it; otherwise the
 * declared value with var() chains resolved. Tailwind v4 emits only the `@theme` variables a
 * class uses, and `@theme inline` none, so the source is the only place the others live.
 * `byVar` maps a CSS variable to its token.
 */
export function resolvedValueOf(token, byVar, style) {
  if (!token.cssVar) return token.value;
  return style.getPropertyValue(token.cssVar).trim() || resolveVars(token.value, byVar, style, new Set([token.cssVar]));
}

const ALIAS = /^\s*var\(\s*(--[\w-]+)\s*(?:,[^]*)?\)\s*$/;

/**
 * An alias whose name says nothing (`--success: var(--emerald-500)`) takes the kind of the token
 * it points to, following var() chains; a kind the name or value already gave is kept. An alias
 * to nothing, or a cycle, stays `other`. A `text` alias whose chain ends at a color
 * (`--text-primary: var(--slate-950)`) is a text color, so it becomes `color`.
 */
export function adoptAliasKinds(tokens) {
  const byVar = new Map();
  for (const token of tokens) if (token.cssVar && !byVar.has(token.cssVar)) byVar.set(token.cssVar, token);
  const kindOf = (token, seen) => {
    if (token.kind !== 'other' && token.kind !== 'text') return token.kind;
    const target = byVar.get(ALIAS.exec(token.value)?.[1]);
    if (!target || seen.has(target)) return token.kind;
    const adopted = kindOf(target, new Set([...seen, target]));
    if (token.kind === 'text') return adopted === 'color' ? 'color' : 'text';
    return adopted;
  };
  return tokens.map((token) => {
    const kind = kindOf(token, new Set([token]));
    return kind === token.kind ? token : { ...token, kind };
  });
}

/** "N tokens: X colors, Y spacing, ..." */
export function summaryLine(tokens) {
  const parts = groupByKind(tokens).map(([kind, list]) => `${list.length} ${kind === 'color' ? 'colors' : kind}`);
  return `${tokens.length} tokens${parts.length ? `: ${parts.join(', ')}` : ''}`;
}
