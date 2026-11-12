import { test } from 'node:test';
import assert from 'node:assert/strict';
import { colorRole, groupColors, hexOf, paletteRefOf, ROLE_LABELS, ROLE_ORDER, themeRefOf } from '../../skills/design-space/templates/app/src/tokens/color-role.mjs';

test('colorRole groups color tokens by role', () => {
  const cases = {
    brand: ['--primary', '--secondary-active', '--accent-alt', '--color-brand'],
    status: ['--success-subtle', '--destructive', '--warning'],
    surface: ['--background', '--card-foreground', '--border', '--ring', '--text-secondary', '--surface', '--sidebar-accent'],
    palette: ['--violet-50', '--slate-950', '--color-gray-100'],
    other: ['--chart-1'],
  };
  for (const [role, names] of Object.entries(cases)) {
    for (const name of names) assert.equal(colorRole(name), role, name);
  }
});

test('role order and labels', () => {
  assert.deepEqual(ROLE_ORDER, ['brand', 'status', 'surface', 'palette', 'other']);
  assert.deepEqual(ROLE_ORDER.map((r) => ROLE_LABELS[r]), ['Brand', 'Status', 'Surface & text', 'Palette', 'Other colors']);
});

test('paletteRefOf reads a var() to a palette token', () => {
  assert.equal(paletteRefOf('var(--violet-600)'), 'violet-600');
  assert.equal(paletteRefOf('#fff'), null);
});

test('colorRole strips the colors- prefix of Tailwind v3 theme-object tokens', () => {
  assert.equal(colorRole('colors-primary'), 'brand');
  assert.equal(colorRole('colors-brand-500'), 'palette');
  assert.equal(colorRole('colors-success'), 'status');
  assert.equal(colorRole('colors-background'), 'surface');
  assert.equal(colorRole('colors-chart-1'), 'other');
});

const tok = (name) => ({ name, kind: 'color', value: '#000', cssVar: `--${name}`, source: 'product', inline: false });
const names = (groups) => groups.map(([role, list]) => [role, list.map((t) => t.name)]);

test('groupColors orders roles, omits empty ones and sorts by name', () => {
  const groups = groupColors(['warning', 'secondary', 'primary', 'chart-1', 'error', 'primary-foreground'].map(tok));
  assert.deepEqual(names(groups), [
    ['brand', ['primary', 'primary-foreground', 'secondary']],
    ['status', ['error', 'warning']],
    ['other', ['chart-1']],
  ]);
});

test('groupColors sorts the palette by family, then by scale number', () => {
  const list = ['slate-950', 'violet-600', 'slate-50', 'violet-50', 'slate-100', 'violet-950', 'slate-500', 'violet-100'];
  assert.deepEqual(names(groupColors(list.map(tok))), [
    ['palette', ['slate-50', 'slate-100', 'slate-500', 'slate-950', 'violet-50', 'violet-100', 'violet-600', 'violet-950']],
  ]);
});

test('groupColors returns nothing for no tokens', () => {
  assert.deepEqual(groupColors([]), []);
});

test('hexOf writes channels as uppercase hex', () => {
  assert.equal(hexOf({ r: 124, g: 58, b: 237 }), '#7C3AED');
  assert.equal(hexOf({ r: 0, g: 10.6, b: 255 }), '#000BFF');
});

test('themeRefOf keeps the declared reference while it still holds the color', () => {
  const hexByName = new Map([['violet-600', '#7C3AED'], ['violet-500', '#8B5CF6'], ['primary', '#7C3AED']]);
  assert.equal(themeRefOf({ ref: 'violet-600', declaredHex: null, hex: '#7C3AED' }, hexByName), 'violet-600');
  assert.equal(themeRefOf({ ref: 'primary', declaredHex: null, hex: '#7C3AED' }, hexByName), 'primary');
});

test('themeRefOf finds the palette token of the active theme when the declared one is stale', () => {
  const hexByName = new Map([['violet-600', '#7C3AED'], ['violet-500', '#8B5CF6'], ['primary', '#8B5CF6'], ['slate-950', '#020617']]);
  // Dark theme: --primary now points at violet-500, --background at slate-950.
  assert.equal(themeRefOf({ ref: 'violet-600', declaredHex: null, hex: '#8B5CF6' }, hexByName), 'violet-500');
  assert.equal(themeRefOf({ ref: null, declaredHex: '#FFFFFF', hex: '#020617' }, hexByName), 'slate-950');
  assert.equal(themeRefOf({ ref: 'violet-600', declaredHex: null, hex: '#123456' }, hexByName), null);
});

test('themeRefOf never invents a reference for a literal that still holds', () => {
  const hexByName = new Map([['slate-100', '#F1F5F9']]);
  assert.equal(themeRefOf({ ref: null, declaredHex: '#F1F5F9', hex: '#F1F5F9' }, hexByName), null);
  assert.equal(themeRefOf({ ref: 'violet-600', declaredHex: null, hex: null }, hexByName), 'violet-600');
  // A reference to a token the map does not know cannot be checked: it is kept.
  assert.equal(themeRefOf({ ref: 'brand-700', declaredHex: null, hex: '#F1F5F9' }, hexByName), 'brand-700');
});
