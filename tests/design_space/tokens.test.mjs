import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseCssTokens, kindOf, flattenTheme, isColorValue, cssColorOf } from '../../skills/design-space/templates/app/src/tokens/parse.mjs';

test('parses :root custom properties', () => {
  const t = parseCssTokens(':root{--color-text:#111;--spacing-4: 1rem}', 'product');
  assert.deepEqual(t, [
    { name: 'color-text', kind: 'color', value: '#111', cssVar: '--color-text', source: 'product', inline: false },
    { name: 'spacing-4', kind: 'spacing', value: '1rem', cssVar: '--spacing-4', source: 'product', inline: false },
  ]);
});

test('@theme inline and static', () => {
  const [a] = parseCssTokens('@theme inline { --color-brand: var(--brand) }', 'product');
  assert.equal(a.inline, true);
  assert.equal(a.value, 'var(--brand)');
  const s = parseCssTokens('@theme static { --radius-lg: 8px }', 'library');
  assert.equal(s.length, 1);
  assert.equal(s[0].inline, false);
  assert.equal(s[0].source, 'library');
});

test('dark overrides are not new tokens; comments ignored', () => {
  const css = `/* --nope: 1 */ :root { --color-bg: #fff }
    .dark { --color-bg: #000; --color-only-dark: #111 }
    @media (prefers-color-scheme: dark) { :root { --color-bg: #000 } }
    @layer base { :root { --color-fg: #222 } }`;
  assert.deepEqual(parseCssTokens(css, 'product').map((t) => t.name), ['color-bg', 'color-fg']);
});

test('kindOf', () => {
  const k = {
    '--color-brand': 'color', '--primary': 'color', '--background': 'color', '--foreground': 'color',
    '--card-foreground': 'color', '--border': 'color', '--chart-1': 'color', '--sidebar-accent': 'color',
    '--spacing': 'spacing', '--spacing-4': 'spacing', '--space-2': 'spacing',
    '--font-sans': 'font', '--text-lg': 'text', '--radius': 'radius', '--radius-md': 'radius',
    '--shadow-sm': 'shadow', '--breakpoint-md': 'breakpoint', '--ease-fluid': 'other',
  };
  for (const [n, kind] of Object.entries(k)) assert.equal(kindOf(n), kind, n);
});

test('flattenTheme', () => {
  const t = flattenTheme({ colors: { primary: { DEFAULT: '#06c', light: '#39f' } }, spacing: { 4: '1rem' } });
  assert.deepEqual(t.map((x) => [x.name, x.kind, x.value, x.cssVar]), [
    ['colors-primary', 'color', '#06c', null],
    ['colors-primary-light', 'color', '#39f', null],
    ['spacing-4', 'spacing', '1rem', null],
  ]);
  assert.ok(t.every((x) => x.source === 'product' && x.inline === false));
});

test('unclassified tokens are kept as other', () => {
  const [t] = flattenTheme({ zIndex: { modal: 50 } });
  assert.equal(t.kind, 'other');
  assert.equal(t.value, '50');
});

const pick = (css) => parseCssTokens(css, 'product').map((t) => [t.name, t.value, t.inline]);

test('tokens after brace-less at-rules are kept', () => {
  assert.deepEqual(pick('@import "tailwindcss";\n@theme { --color-a: #fff; }'), [['color-a', '#fff', false]]);
  assert.deepEqual(
    pick('@import "tailwindcss";\n@custom-variant dark (&:is(.dark *));\n:root { --primary: #000 }\n@theme inline { --color-primary: var(--primary) }'),
    [['primary', '#000', false], ['color-primary', 'var(--primary)', true]],
  );
  assert.deepEqual(pick('@layer theme, base;\n@theme { --color-a: #fff; }'), [['color-a', '#fff', false]]);
});

import { groupByKind, mergeTokens, liveValueOf, summaryLine } from '../../skills/design-space/templates/app/src/tokens/map-core.mjs';

const tok = (name, kind, cssVar = `--${name}`, source = 'product') => ({ name, kind, value: 'v', cssVar, source, inline: false });

test('groupByKind uses the fixed order, keeps other, omits empty kinds', () => {
  const groups = groupByKind([tok('z', 'other'), tok('r', 'radius'), tok('c', 'color'), tok('t', 'text'), tok('s', 'spacing')]);
  assert.deepEqual(groups.map(([k]) => k), ['color', 'spacing', 'text', 'radius', 'other']);
  assert.equal(groups.at(-1)[1][0].name, 'z');
});

test('mergeTokens de-duplicates by cssVar, product first wins', () => {
  const merged = mergeTokens([tok('a', 'color')], [tok('a', 'color', '--a', 'library'), tok('b', 'color', '--b', 'library')]);
  assert.deepEqual(merged.map((t) => [t.name, t.source]), [['a', 'product'], ['b', 'library']]);
});

test('mergeTokens de-duplicates by name when cssVar is null, and keeps distinct nulls', () => {
  const merged = mergeTokens([tok('x', 'color', null)], [tok('x', 'color', null, 'library'), tok('y', 'color', null, 'library')]);
  assert.deepEqual(merged.map((t) => t.name), ['x', 'y']);
  assert.equal(merged[0].source, 'product');
});

test('liveValueOf falls back to the declared value', () => {
  const style = { getPropertyValue: (n) => (n === '--a' ? ' red ' : '') };
  assert.equal(liveValueOf(tok('a', 'color'), style), 'red');
  assert.equal(liveValueOf(tok('b', 'color'), style), 'v');
  assert.equal(liveValueOf(tok('c', 'color', null), style), 'v');
});

test('summaryLine counts per kind', () => {
  assert.equal(summaryLine([tok('a', 'color'), tok('b', 'color'), tok('c', 'spacing')]), '3 tokens: 2 colors, 1 spacing');
});

import { resolveVars, resolvedValueOf } from '../../skills/design-space/templates/app/src/tokens/map-core.mjs';

// Tailwind v4 plain `@theme` emits only the variables a class uses: the others read empty.
const emitted = (values) => ({ getPropertyValue: (n) => values[n] ?? '' });

test('resolvedValueOf: an emitted variable is read live', () => {
  const t = { ...tok('color-a', 'color'), value: '#123' };
  assert.equal(resolvedValueOf(t, new Map(), emitted({ '--color-a': ' #fff ' })), '#fff');
});

test('resolvedValueOf: a variable not emitted falls back to its declared value, resolving var() chains', () => {
  const tokens = [
    { ...tok('color-primary', 'color'), value: 'var(--color-blue-500)' },
    { ...tok('color-blue-500', 'color'), value: 'oklch(0.6 0.2 250)' },
    { ...tok('color-bg', 'color'), value: 'var(--background)' },
    { ...tok('color-fb', 'color'), value: 'var(--nope, var(--color-blue-500))' },
    { ...tok('color-loop', 'color'), value: 'var(--color-loop)' },
  ];
  const byVar = new Map(tokens.map((t) => [t.cssVar, t]));
  const style = emitted({ '--background': '#000' });
  const [primary, , bg, fb, loop] = tokens.map((t) => resolvedValueOf(t, byVar, style));
  assert.equal(primary, 'oklch(0.6 0.2 250)');
  assert.equal(bg, '#000');
  assert.equal(fb, 'oklch(0.6 0.2 250)');
  assert.equal(loop, 'var(--color-loop)');
  assert.equal(resolvedValueOf({ ...tok('x', 'color', null), value: 'red' }, byVar, style), 'red');
});

test('resolveVars keeps what it cannot resolve and resolves inside functions', () => {
  const style = emitted({ '--a': '10px' });
  assert.equal(resolveVars('calc(var(--a) * 2)', new Map(), style), 'calc(10px * 2)');
  assert.equal(resolveVars('var(--missing)', new Map(), style), 'var(--missing)');
  assert.equal(resolveVars('var(--missing, 4px)', new Map(), style), '4px');
});

import { mergeLibrary } from '../../skills/design-space/templates/app/src/tokens/map-core.mjs';

test('mergeLibrary keeps a library token that redefines a product variable, flagged overrides', () => {
  const product = [tok('color-bg', 'color'), tok('colors-x', 'color', null)];
  const library = [
    tok('color-bg', 'color', '--color-bg', 'library'),
    tok('color-new', 'color', '--color-new', 'library'),
    tok('color-bg', 'color', '--color-bg', 'library'),
  ];
  const merged = mergeLibrary(product, library);
  assert.deepEqual(merged.map((t) => [t.name, t.source, t.overrides ?? false]), [
    ['color-bg', 'product', false],
    ['colors-x', 'product', false],
    ['color-bg', 'library', true],
    ['color-new', 'library', false],
  ]);
});

test('kindOf classifies by value when the name does not say', () => {
  assert.equal(kindOf('--violet-500', '#8b5cf6'), 'color');
  assert.equal(kindOf('--success', 'oklch(0.7 0.1 150)'), 'color');
  assert.equal(kindOf('--text-primary', '#111'), 'color');
  assert.equal(kindOf('--text-lg', '1.125rem'), 'text');
  assert.equal(kindOf('--ease-out', 'cubic-bezier(0,0,.2,1)'), 'other');
  assert.equal(kindOf('--violet-500'), 'other');
});

test('isColorValue recognizes color formats and bare HSL channels', () => {
  for (const v of ['#fff', '#ffff', '#ffffff', '#ffffff80', 'rgb(1 2 3)', 'rgba(1,2,3,.5)', 'hsl(0 0% 0%)', 'hsla(0,0%,0%,.5)',
    'oklch(0.7 0.1 150)', 'oklab(0.7 0.1 0.1)', 'lab(50% 1 1)', 'lch(50% 1 1)', 'color(display-p3 1 0 0)',
    '0 0% 100%', '222.2 84% 4.9%', '222.2 84% 4.9% / 0.5']) assert.equal(isColorValue(v), true, v);
  for (const v of ['1rem', 'transparent', 'currentColor', 'var(--x)', '#ff', 'cubic-bezier(0,0,.2,1)', '']) assert.equal(isColorValue(v), false, v);
});

test('cssColorOf wraps bare HSL channels only', () => {
  assert.equal(cssColorOf('222.2 84% 4.9%'), 'hsl(222.2 84% 4.9%)');
  assert.equal(cssColorOf('#fff'), '#fff');
  assert.equal(cssColorOf('1rem'), '1rem');
});

test('parseCssTokens and flattenTheme classify by value', () => {
  assert.equal(parseCssTokens(':root { --violet-500: #8b5cf6 }', 'product')[0].kind, 'color');
  assert.equal(flattenTheme({ brand: { x: '#123456' } })[0].kind, 'color');
});

import { adoptAliasKinds } from '../../skills/design-space/templates/app/src/tokens/map-core.mjs';

test('adoptAliasKinds: an unclassified alias takes the kind of the token it points to', () => {
  const css = ':root { --emerald-500: #10b981; --success: var(--emerald-500); --ok: var(--success); --z-overlay: var(--z-missing); --gap: var(--spacing-4); --loop-a: var(--loop-b); --loop-b: var(--loop-a); }\n@theme { --spacing-4: 1rem; }';
  const tokens = adoptAliasKinds(parseCssTokens(css, 'product'));
  const kind = Object.fromEntries(tokens.map((t) => [t.name, t.kind]));
  assert.equal(kind.success, 'color');
  assert.equal(kind.ok, 'color');
  assert.equal(kind['z-overlay'], 'other');
  assert.equal(kind.gap, 'spacing');
  assert.equal(kind['loop-a'], 'other');
  assert.equal(kind['loop-b'], 'other');
});

test('adoptAliasKinds keeps a kind the name already gives', () => {
  const tokens = adoptAliasKinds(parseCssTokens(':root { --a: #fff; --radius-md: var(--a); }', 'product'));
  assert.equal(tokens.find((t) => t.name === 'radius-md').kind, 'radius');
});

test('adoptAliasKinds: a text-named alias of a color becomes a color', () => {
  const css = ':root { --slate-950: #020617; --text-primary: var(--slate-950); --text-muted: var(--text-primary); --text-lg: var(--size-lg); --size-lg: 1.125rem; }';
  const kind = Object.fromEntries(adoptAliasKinds(parseCssTokens(css, 'product')).map((t) => [t.name, t.kind]));
  assert.equal(kind['text-primary'], 'color');
  assert.equal(kind['text-muted'], 'color');
  assert.equal(kind['text-lg'], 'text');
});
