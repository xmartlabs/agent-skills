import { test } from 'node:test';
import assert from 'node:assert/strict';
import { declaredTheme } from '../../skills/design-space/templates/tailwind-v3/tailwind-theme.mjs';

// A stand-in for tailwindcss/resolveConfig: Tailwind's defaults unless `presets: []`, then the
// product's theme with `extend` merged in and functions called with a theme() helper.
const DEFAULTS = { colors: { blue: { 500: '#3b82f6' }, white: '#fff' }, spacing: { 4: '1rem' } };
const get = (obj, path) => path.split('.').reduce((n, k) => n?.[k], obj);
function resolveConfig(config) {
  const base = Array.isArray(config.presets) && config.presets.length === 0 ? {} : structuredClone(DEFAULTS);
  const { extend = {}, ...own } = config.theme ?? {};
  const theme = { ...base };
  const helper = (path) => get(theme, path);
  for (const [key, value] of Object.entries(own)) theme[key] = typeof value === 'function' ? value({ theme: helper }) : value;
  for (const [key, value] of Object.entries(extend)) {
    const v = typeof value === 'function' ? value({ theme: helper }) : value;
    theme[key] = { ...(theme[key] ?? {}), ...v };
  }
  return { theme };
}

test('declaredTheme keeps only what the product declares, extend merged in', () => {
  const config = { theme: { extend: { colors: { success: '#388e3c', error: '#d32f2f' } } } };
  assert.deepEqual(declaredTheme(config, resolveConfig), { colors: { success: '#388e3c', error: '#d32f2f' } });
});

test('declaredTheme keeps a full theme override and its own keys', () => {
  const config = { theme: { colors: { white: '#fff', ink: '#111' }, extend: { spacing: { 18: '4.5rem' } } } };
  assert.deepEqual(declaredTheme(config, resolveConfig), {
    colors: { white: '#fff', ink: '#111' },
    spacing: { 18: '4.5rem' },
  });
});

test('declaredTheme resolves values the product computes from the default theme', () => {
  const config = { theme: { extend: { colors: ({ theme }) => ({ primary: theme('colors.blue.500') }) } } };
  assert.deepEqual(declaredTheme(config, resolveConfig), { colors: { primary: '#3b82f6' } });
});

test('declaredTheme keeps a product preset: it is the product\'s theme', () => {
  const preset = { theme: { colors: { brand: '#06c' } } };
  const withPreset = (config) => (config.presets?.length ? { theme: { ...config.presets[0].theme, ...config.theme } } : resolveConfig(config));
  assert.deepEqual(declaredTheme({ presets: [preset], theme: {} }, withPreset), { colors: { brand: '#06c' } });
});
