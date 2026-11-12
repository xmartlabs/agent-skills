import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import {
  stripFolderSourceNot, tailwindV4Source,
} from '../../skills/design-space/templates/app/build/tailwind-v4-source.mjs';

test('drops the @source not line for the design space folder and keeps the import', () => {
  assert.equal(
    stripFolderSourceNot('@import "tailwindcss";\n@source not "../../design-space";\n', 'design-space'),
    '@import "tailwindcss";\n',
  );
});

test('keeps an @source not line for another folder', () => {
  const css = '@import "tailwindcss";\n@source not "../other";\n';
  assert.equal(stripFolderSourceNot(css, 'design-space'), css);
});

test('keeps a positive @source line for the folder', () => {
  const css = '@import "tailwindcss";\n@source "../../design-space";\n';
  assert.equal(stripFolderSourceNot(css, 'design-space'), css);
});

test('matches single quotes, a trailing slash or glob, and a nested folder', () => {
  const css = "@source not '../tools/design-space/';\n@source not \"../tools/design-space/**\";\na{}\n";
  assert.equal(stripFolderSourceNot(css, 'tools/design-space'), 'a{}\n');
});

test('does not match a folder that only ends with the same name', () => {
  const css = '@source not "../my-design-space";\n';
  assert.equal(stripFolderSourceNot(css, 'design-space'), css);
});

test('with the stylesheet folder, compares resolved paths', () => {
  const css = '@source not "../design-space";\n@source not "../../elsewhere/design-space";\n';
  assert.equal(
    stripFolderSourceNot(css, '/repo/design-space', '/repo/src'),
    '@source not "../../elsewhere/design-space";\n',
  );
});

test('the plugin runs before Tailwind and transforms only the product stylesheets', () => {
  const root = '/repo';
  const plugin = tailwindV4Source({ root, folder: join(root, 'design-space'), stylesheets: ['src/app.css'] });
  assert.equal(plugin.enforce, 'pre');
  const css = '@import "tailwindcss";\n@source not "../design-space";\n';
  const out = plugin.transform(css, join(root, 'src/app.css'));
  assert.equal(out, '@import "tailwindcss";\n@source "..";\n');
  assert.equal(plugin.transform(css, join(root, 'src/app.css?raw')), undefined);
  assert.equal(plugin.transform(css, join(root, 'src/other.css')), undefined);
});

test('warns once at buildEnd when no listed stylesheet was transformed', () => {
  const root = '/repo';
  const warnings = [];
  const ctx = { warn: (message) => warnings.push(message) };
  const idle = tailwindV4Source({ root, folder: join(root, 'design-space'), stylesheets: ['src/app.css'] });
  idle.buildEnd.call(ctx);
  idle.buildEnd.call(ctx);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /src\/app\.css/);
  const used = tailwindV4Source({ root, folder: join(root, 'design-space'), stylesheets: ['src/app.css'] });
  used.transform('@import "tailwindcss";\n', join(root, 'src/app.css'));
  used.buildEnd.call(ctx);
  assert.equal(warnings.length, 1);
});
