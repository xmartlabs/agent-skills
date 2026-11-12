import { test } from 'node:test';
import assert from 'node:assert/strict';
import { proposeContrastPairs, colorVisionPairs, tokenUsageFindings } from '../../skills/design-space/templates/app/src/audits/pairs.mjs';

const tok = (cssVar, value = '#000', kind = 'color') => ({
  name: cssVar.replace(/^--/, ''), kind, value, cssVar, source: 'product', inline: false,
});

test('proposeContrastPairs: foreground on its surface, text on background, muted on background', () => {
  const tokens = [tok('--primary'), tok('--primary-foreground'), tok('--background'), tok('--foreground'), tok('--muted-foreground')];
  assert.deepEqual(proposeContrastPairs(tokens), [
    { foreground: '--primary-foreground', background: '--primary', label: 'primary-foreground on primary', large: false },
    { foreground: '--foreground', background: '--background', label: 'foreground on background', large: false },
    { foreground: '--muted-foreground', background: '--background', label: 'muted-foreground on background', large: false },
  ]);
});

test('proposeContrastPairs: Tailwind v4 names pair muted text with the background too', () => {
  const tokens = [tok('--color-background'), tok('--color-foreground'), tok('--color-muted-foreground')];
  assert.deepEqual(proposeContrastPairs(tokens), [
    { foreground: '--color-foreground', background: '--color-background', label: 'foreground on background', large: false },
    { foreground: '--color-muted-foreground', background: '--color-background', label: 'muted-foreground on background', large: false },
  ]);
});

test('proposeContrastPairs: nothing to pair gives an empty list', () => {
  assert.deepEqual(proposeContrastPairs([tok('--radius', '4px', 'radius')]), []);
  assert.deepEqual(proposeContrastPairs([]), []);
});

test('colorVisionPairs: contrast pairs plus status and chart colors', () => {
  const tokens = [tok('--success'), tok('--destructive'), tok('--primary'), tok('--radius', '4px', 'radius')];
  const pairs = colorVisionPairs(tokens, [{ foreground: '--foreground', background: '--background', label: 'x', large: false }]);
  assert.deepEqual(pairs, [{ a: '--foreground', b: '--background' }, { a: '--success', b: '--destructive' }]);
});

test('tokenUsageFindings: empty values are unresolved, shared values are duplicates', () => {
  const tokens = [tok('--a', '#06c'), tok('--b', '#06c'), tok('--c', ''), tok('--d', '#fff')];
  const values = new Map([['a', '#06c'], ['b', '#06C'], ['c', ''], ['d', '#fff']]);
  assert.deepEqual(tokenUsageFindings(tokens, values), {
    unresolved: ['c'],
    duplicates: [{ value: '#06c', names: ['a', 'b'] }],
    overrides: [],
  });
});

test('colorVisionPairs: a theme-object token (no cssVar) pairs by name with a CSS status token', () => {
  const themed = { name: 'colors-success-500', kind: 'color', value: '#0a0', cssVar: null, source: 'product', inline: false };
  const pairs = colorVisionPairs([themed, tok('--destructive')], []);
  assert.deepEqual(pairs, [{ a: 'colors-success-500', b: '--destructive' }]);
});

test('colorVisionPairs: library-source tokens are included', () => {
  const lib = { ...tok('--warning'), source: 'library' };
  assert.deepEqual(colorVisionPairs([lib, tok('--error')], []), [{ a: '--warning', b: '--error' }]);
});

import { parseCssTokens } from '../../skills/design-space/templates/app/src/tokens/parse.mjs';

// shadcn on Tailwind v4: `@theme inline` aliases every `:root` variable under a `--color-` name.
const SHADCN = `@import "tailwindcss";
@custom-variant dark (&:is(.dark *));
@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-muted-foreground: var(--muted-foreground);
  --color-destructive: var(--destructive);
  --color-chart-1: var(--chart-1);
  --color-chart-2: var(--chart-2);
}
:root {
  --background: oklch(1 0 0);
  --foreground: oklch(0.145 0 0);
  --primary: oklch(0.205 0 0);
  --primary-foreground: oklch(0.985 0 0);
  --muted-foreground: oklch(0.556 0 0);
  --destructive: oklch(0.577 0.245 27.325);
  --chart-1: oklch(0.646 0.222 41.116);
  --chart-2: oklch(0.6 0.118 184.704);
}
.dark { --background: oklch(0.145 0 0); }
`;

test('proposeContrastPairs: shadcn aliases give one pair per meaning, on the emitted variables', () => {
  assert.deepEqual(proposeContrastPairs(parseCssTokens(SHADCN, 'product')), [
    { foreground: '--primary-foreground', background: '--primary', label: 'primary-foreground on primary', large: false },
    { foreground: '--foreground', background: '--background', label: 'foreground on background', large: false },
    { foreground: '--muted-foreground', background: '--background', label: 'muted-foreground on background', large: false },
  ]);
});

test('colorVisionPairs: shadcn aliases never pair a token with its own alias, and pairs are unique', () => {
  const tokens = parseCssTokens(SHADCN, 'product');
  assert.deepEqual(colorVisionPairs(tokens, proposeContrastPairs(tokens)), [
    { a: '--primary-foreground', b: '--primary' },
    { a: '--foreground', b: '--background' },
    { a: '--muted-foreground', b: '--background' },
    { a: '--destructive', b: '--chart-1' },
    { a: '--destructive', b: '--chart-2' },
    { a: '--chart-1', b: '--chart-2' },
  ]);
});

test('colorVisionPairs: contrast pairs naming aliases are mapped and de-duplicated', () => {
  const tokens = parseCssTokens(SHADCN, 'product');
  const contrast = [
    { foreground: '--color-foreground', background: '--color-background', label: 'x', large: false },
    { foreground: '--foreground', background: '--background', label: 'y', large: false },
    { foreground: '--color-background', background: '--background', label: 'self', large: false },
  ];
  assert.deepEqual(colorVisionPairs(tokens, contrast), [
    { a: '--foreground', b: '--background' },
    { a: '--destructive', b: '--chart-1' },
    { a: '--destructive', b: '--chart-2' },
    { a: '--chart-1', b: '--chart-2' },
  ]);
});

test('aliases chain through several tokens; an inline alias maps to the first emitted one', () => {
  const tokens = [
    tok('--success', '#0a0'), tok('--color-success', 'var(--success)'),
    { ...tok('--color-ok', 'var(--color-success)'), inline: true },
    tok('--error', '#a00'),
  ];
  assert.deepEqual(colorVisionPairs(tokens, []), [
    { a: '--success', b: '--error' },
  ]);
  const contrast = [{ foreground: '--color-ok', background: '--error', label: 'x', large: false }];
  assert.deepEqual(colorVisionPairs(tokens, contrast), [
    { a: '--color-success', b: '--error' },
  ]);
});

test('tokenUsageFindings: a non-inline token whose var() resolves nowhere is unresolved', () => {
  const tokens = [
    tok('--broken', 'var(--nope)', 'other'),
    tok('--fine', 'var(--a)', 'other'),
    tok('--static', '#123'),
    { ...tok('--color-x', 'var(--gone)'), inline: true },
  ];
  // Live (computed) values: empty when not emitted or invalid.
  const live = new Map([['broken', ''], ['fine', ''], ['static', ''], ['color-x', '']]);
  const resolved = new Map([['broken', 'var(--nope)'], ['fine', '4px'], ['static', '#123'], ['color-x', 'var(--gone)']]);
  assert.deepEqual(tokenUsageFindings(tokens, live, resolved), { unresolved: ['broken'], duplicates: [], overrides: [] });
});

test('tokenUsageFindings: a designer token overriding a product token is a finding of its own', () => {
  const tokens = [tok('--bg', '#fff'), { ...tok('--bg', '#000'), source: 'library', overrides: true }];
  const values = new Map([['bg', '#000']]);
  assert.deepEqual(tokenUsageFindings(tokens, values), { unresolved: [], duplicates: [], overrides: ['bg'] });
});
