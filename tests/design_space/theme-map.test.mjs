import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildThemeMap, SHELL_VARS, NEUTRAL, CANDIDATES } from '../../skills/design-space/templates/app/src/shell/theme-map-core.mjs';

const tok = (cssVar, value) => ({ name: cssVar.slice(2), kind: 'other', value, cssVar, source: 'product', inline: false });

test('shell vars are the agreed twelve and each has candidates and a neutral', () => {
  assert.equal(SHELL_VARS.length, 12);
  for (const name of SHELL_VARS) {
    assert.ok(CANDIDATES[name].length > 0, name);
    assert.ok(NEUTRAL.light[name], name);
  }
});

test('no tokens: every var falls back to its neutral value', () => {
  const { css, mapped } = buildThemeMap([], { kind: 'none' });
  assert.match(css, /--ds-bg: #ffffff;/);
  for (const name of SHELL_VARS) {
    assert.ok(css.includes(`${name}:`), name);
    assert.equal(mapped[name], null);
  }
  assert.doesNotMatch(css, /\.dark|\[data-theme/);
});

test('bare HSL channels are wrapped in hsl()', () => {
  const { css, mapped } = buildThemeMap([tok('--background', '0 0% 100%')], { kind: 'none' });
  assert.match(css, /--ds-bg: hsl\(var\(--background, 0 0% 100%\)\);/);
  assert.equal(mapped['--ds-bg'], '--background');
});

test('other values keep the product variable with the neutral as fallback', () => {
  const { css } = buildThemeMap([tok('--color-primary', '#7c3aed')], { kind: 'none' });
  assert.ok(css.includes(`--ds-primary: var(--color-primary, ${NEUTRAL.light['--ds-primary']});`));
});

test('the first candidate present wins', () => {
  const { mapped } = buildThemeMap([tok('--surface', '#fff'), tok('--background', '#eee')], { kind: 'none' });
  assert.equal(mapped['--ds-bg'], '--background');
});

test('fonts map to the product fonts', () => {
  const { css, mapped } = buildThemeMap([tok('--font-sans', 'Inter, sans-serif'), tok('--font-display', 'Lora')], { kind: 'none' });
  assert.equal(mapped['--ds-font'], '--font-sans');
  assert.equal(mapped['--ds-font-heading'], '--font-display');
  assert.match(css, /--ds-font-heading: var\(--font-display, /);
});

test('class mechanism: neutral dark values only for vars that fell back', () => {
  const { css } = buildThemeMap([tok('--background', '#fff')], { kind: 'class', className: 'dark' });
  const dark = css.split('\n.dark {\n')[1];
  assert.ok(dark, css);
  assert.ok(!dark.includes('--ds-bg:'));
  assert.ok(dark.includes(`--ds-fg: ${NEUTRAL.dark['--ds-fg']};`));
});

test('attribute mechanism uses the attribute selector', () => {
  const { css } = buildThemeMap([], { kind: 'attribute', attribute: 'data-theme', dark: 'dark', light: 'light' });
  assert.match(css, /\n\[data-theme="dark"\] \{\n/);
});

test('media mechanism: neutral dark values under prefers-color-scheme, only for vars that fell back', () => {
  const { css } = buildThemeMap([tok('--background', '#fff')], { kind: 'media' });
  const dark = css.split('\n@media (prefers-color-scheme: dark) {\n  :root {\n')[1];
  assert.ok(dark, css);
  assert.ok(!dark.includes('--ds-bg:'));
  assert.ok(dark.includes(`    --ds-fg: ${NEUTRAL.dark['--ds-fg']};`));
  assert.ok(dark.trimEnd().endsWith('  }\n}'), css);
});

test('no mechanism emits no dark block', () => {
  assert.doesNotMatch(buildThemeMap([], { kind: 'none' }).css, /\n[.\[@][^{]*\{\n/);
});

test('everything mapped: no dark block', () => {
  const tokens = SHELL_VARS.flatMap((v) => (CANDIDATES[v][0] ? [tok(CANDIDATES[v][0], '#123456')] : []));
  const { css } = buildThemeMap(tokens, { kind: 'class', className: 'dark' });
  assert.doesNotMatch(css, /\.dark/);
});

test('heading falls back to the body font, not the system stack', () => {
  const { css } = buildThemeMap([tok('--font-sans', 'Inter')], { kind: 'none' });
  assert.match(css, /--ds-font-heading: var\(--ds-font\);/);
  const mappedHeading = buildThemeMap([tok('--font-heading', 'Lora')], { kind: 'none' }).css;
  assert.match(mappedHeading, /--ds-font-heading: var\(--font-heading, var\(--ds-font\)\);/);
});

test('detected fonts become fallbacks after the product variable', () => {
  const { css } = buildThemeMap([tok('--font-sans', 'var(--font-inter)')], { kind: 'none' },
    { fonts: ['Playfair Display', 'Inter'] });
  assert.ok(css.includes(`--ds-font: var(--font-sans, "Inter", ${NEUTRAL.light['--ds-font']});`), css);
  assert.match(css, /--ds-font-heading: "Playfair Display", var\(--ds-font\);/);
});

test('detected fonts with a heading token and a single family', () => {
  const { css } = buildThemeMap([tok('--font-heading', 'x')], { kind: 'none' }, { fonts: ['Inter'] });
  assert.match(css, /--ds-font-heading: var\(--font-heading, "Inter", var\(--ds-font\)\);/);
});

test('a candidate whose value is not a color is skipped for color vars', () => {
  const { css, mapped } = buildThemeMap([tok('--background', 'url(x.png)'), tok('--surface', '#fafafa')], { kind: 'none' });
  assert.equal(mapped['--ds-bg'], '--surface');
  assert.match(css, /--ds-bg: var\(--surface, /);
  assert.equal(buildThemeMap([tok('--primary', 'var(--brand)')], { kind: 'none' }).mapped['--ds-primary'], '--primary');
});
