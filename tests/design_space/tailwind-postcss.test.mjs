import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import {
  postcssPlugins, tailwindPostcss, withDesignSpaceContent,
} from '../../skills/design-space/templates/tailwind-v3/tailwind-postcss.mjs';

// A repository with stand-ins for tailwindcss and autoprefixer that record how they were called.
function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), 'twpc-'));
  const all = {
    'package.json': '{"name":"acme-web-app"}',
    'node_modules/tailwindcss/package.json': '{"name":"tailwindcss","main":"index.js"}',
    'node_modules/tailwindcss/index.js':
      'module.exports = function tailwindcss(config) { return { postcssPlugin: "tailwindcss", config }; };\nmodule.exports.postcss = true;\n',
    'node_modules/autoprefixer/package.json': '{"name":"autoprefixer","main":"index.js"}',
    'node_modules/autoprefixer/index.js':
      'module.exports = function autoprefixer(options) { return { postcssPlugin: "autoprefixer", options }; };\n',
    'tailwind.config.js': 'module.exports = { content: ["./src/**/*.tsx", "!./src/legacy/**"], theme: {} };\n',
    ...files,
  };
  for (const [rel, body] of Object.entries(all)) {
    mkdirSync(dirname(join(root, rel)), { recursive: true });
    writeFileSync(join(root, rel), body);
  }
  return root;
}

const designSpaceGlobs = (appRoot) => ['specimens', 'library', 'flows'].map((d) => `${appRoot}/${d}/**/*.{js,jsx,ts,tsx}`);

test('withDesignSpaceContent: array content becomes absolute and gains the design space folders', () => {
  const config = withDesignSpaceContent({ content: ['./src/**/*.tsx', '!./src/old/**', '/abs/x.html', { raw: '<b class="p-1">' }] }, { base: '/repo', appRoot: '/repo/design-space' });
  assert.deepEqual(config.content, [
    '/repo/src/**/*.tsx', '!/repo/src/old/**', '/abs/x.html', { raw: '<b class="p-1">' },
    ...designSpaceGlobs('/repo/design-space'),
  ]);
});

test('withDesignSpaceContent: object content keeps its options; relative globs follow `relative`', () => {
  const config = withDesignSpaceContent(
    { content: { relative: true, files: ['./src/**/*.tsx'], extract: { md: 'x' } } },
    { base: '/repo', configDir: '/repo/web', appRoot: '/repo/design-space' },
  );
  assert.deepEqual(config.content, {
    relative: true,
    extract: { md: 'x' },
    files: ['/repo/web/src/**/*.tsx', ...designSpaceGlobs('/repo/design-space')],
  });
});

test('withDesignSpaceContent: no content scans the design space folders only', () => {
  assert.deepEqual(withDesignSpaceContent({}, { base: '/r', appRoot: '/r/ds' }).content, designSpaceGlobs('/r/ds'));
});

test('postcssPlugins: without a PostCSS config, Tailwind alone with the product config', async () => {
  const root = fixture({});
  try {
    const plugins = await postcssPlugins({ root, appRoot: join(root, 'design-space'), configFile: 'tailwind.config.js' });
    assert.equal(plugins.length, 1);
    assert.equal(plugins[0].postcssPlugin, 'tailwindcss');
    assert.deepEqual(plugins[0].config.content, [
      `${root}/src/**/*.tsx`, `!${root}/src/legacy/**`, ...designSpaceGlobs(join(root, 'design-space')),
    ]);
  } finally { rmSync(root, { recursive: true }); }
});

test('postcssPlugins: object-form PostCSS config keeps every plugin and its options', async () => {
  const root = fixture({ 'postcss.config.js': 'module.exports = { plugins: { tailwindcss: {}, autoprefixer: { grid: true }, skipped: false } };\n' });
  try {
    const plugins = await postcssPlugins({ root, appRoot: join(root, 'design-space'), configFile: 'tailwind.config.js' });
    assert.deepEqual(plugins.map((p) => p.postcssPlugin), ['tailwindcss', 'autoprefixer']);
    assert.ok(plugins[0].config.content.includes(`${join(root, 'design-space')}/specimens/**/*.{js,jsx,ts,tsx}`));
    assert.deepEqual(plugins[1].options, { grid: true });
  } finally { rmSync(root, { recursive: true }); }
});

test('postcssPlugins: object-form tailwindcss options can name another config file', async () => {
  const root = fixture({
    'postcss.config.cjs': 'module.exports = { plugins: { tailwindcss: { config: "./web/tw.config.js" } } };\n',
    'web/tw.config.js': 'module.exports = { content: { relative: true, files: ["./src/**/*.tsx"] } };\n',
  });
  try {
    const [tw] = await postcssPlugins({ root, appRoot: join(root, 'design-space'), configFile: 'tailwind.config.js' });
    assert.equal(tw.config.content.files[0], `${root}/web/src/**/*.tsx`);
  } finally { rmSync(root, { recursive: true }); }
});

test('postcssPlugins: array-form PostCSS config swaps the Tailwind entry, invoked or not', async () => {
  const root = fixture({
    'postcss.config.js': 'module.exports = { plugins: [require("tailwindcss"), require("autoprefixer")()] };\n',
  });
  try {
    const plugins = await postcssPlugins({ root, appRoot: join(root, 'design-space'), configFile: 'tailwind.config.js' });
    assert.deepEqual(plugins.map((p) => p.postcssPlugin), ['tailwindcss', 'autoprefixer']);
    assert.ok(Array.isArray(plugins[0].config.content));
  } finally { rmSync(root, { recursive: true }); }
  const invoked = fixture({ 'postcss.config.js': 'module.exports = { plugins: [require("tailwindcss")()] };\n' });
  try {
    const [tw] = await postcssPlugins({ root: invoked, appRoot: join(invoked, 'design-space'), configFile: 'tailwind.config.js' });
    assert.ok(tw.config.content.some((g) => typeof g === 'string' && g.endsWith('/specimens/**/*.{js,jsx,ts,tsx}')));
  } finally { rmSync(invoked, { recursive: true }); }
});

test('postcssPlugins: a function PostCSS config is called with the environment', async () => {
  const root = fixture({ 'postcss.config.js': 'module.exports = (ctx) => ({ plugins: { tailwindcss: {}, autoprefixer: { env: ctx.env } } });\n' });
  try {
    const plugins = await postcssPlugins({ root, appRoot: join(root, 'design-space'), configFile: 'tailwind.config.js' });
    assert.equal(plugins.length, 2);
    assert.equal(typeof plugins[1].options.env, 'string');
  } finally { rmSync(root, { recursive: true }); }
});

test('tailwindPostcss: a Vite plugin that sets css.postcss', async () => {
  const root = fixture({});
  try {
    const plugin = tailwindPostcss({ root, appRoot: join(root, 'design-space'), configFile: 'tailwind.config.js' });
    assert.equal(plugin.name, 'design-space:tailwind-postcss');
    const config = await plugin.config();
    assert.equal(config.css.postcss.plugins[0].postcssPlugin, 'tailwindcss');
  } finally { rmSync(root, { recursive: true }); }
});
