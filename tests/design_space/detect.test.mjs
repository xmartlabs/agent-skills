import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { detectProduct } from '../../skills/design-space/scripts/detect.mjs';

function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), 'detect-'));
  for (const [rel, body] of Object.entries(files)) {
    if (rel.endsWith('/')) { mkdirSync(join(root, rel), { recursive: true }); continue; }
    mkdirSync(dirname(join(root, rel)), { recursive: true });
    writeFileSync(join(root, rel), body);
  }
  return root;
}
const pkg = (deps, group = 'dependencies') => JSON.stringify({ [group]: deps });

test('tailwind 4 with an @theme stylesheet', () => {
  const root = fixture({
    'package.json': pkg({ tailwindcss: '^4.1.0' }, 'devDependencies'),
    'src/app.css': '@import "tailwindcss"; @theme { --color-primary: #06c; }',
    'src/plain.css': '.a { color: red }',
  });
  try {
    const r = detectProduct(root);
    assert.equal(r.tailwind, 4);
    assert.deepEqual(r.stylesheets, ['src/app.css']);
    assert.equal(r.tailwindConfig, null);
    assert.deepEqual(r.theme, { kind: 'none' });
  } finally { rmSync(root, { recursive: true }); }
});

test('tailwind 3 with a config file', () => {
  const root = fixture({
    'package.json': pkg({ tailwindcss: '^3.4.0' }),
    'tailwind.config.js': 'module.exports = {}',
  });
  try {
    const r = detectProduct(root);
    assert.equal(r.tailwind, 3);
    assert.equal(r.tailwindConfig, 'tailwind.config.js');
  } finally { rmSync(root, { recursive: true }); }
});

test('no tailwind and no package.json', () => {
  const root = fixture({ 'src/a.css': ':root { --x: 1 }' });
  try {
    const r = detectProduct(root);
    assert.equal(r.tailwind, null);
    assert.deepEqual(r.stylesheets, ['src/a.css']);
  } finally { rmSync(root, { recursive: true }); }
});

test('theme mechanism detection', () => {
  const cases = [
    ['.dark { --color-bg: #000 }', { kind: 'class', className: 'dark' }],
    ['[data-theme="dark"] { --color-bg: #000 }', { kind: 'attribute', attribute: 'data-theme', dark: 'dark', light: 'light' }],
    ['@media (prefers-color-scheme: dark) { :root { --color-bg: #000 } }', { kind: 'media' }],
    [':root { --color-bg: #fff }', { kind: 'none' }],
  ];
  for (const [css, expected] of cases) {
    const root = fixture({ 'src/a.css': css });
    try { assert.deepEqual(detectProduct(root).theme, expected, css); }
    finally { rmSync(root, { recursive: true }); }
  }
});

test('components candidates; node_modules and the design space are never scanned', () => {
  const root = fixture({
    'src/components/ui/': '',
    'components/ui/': '',
    'node_modules/pkg/components/ui/': '',
    'node_modules/pkg/a.css': ':root { --x: 1 }',
    'design-space/src/components/ui/': '',
    'design-space/src/a.css': ':root { --y: 1 }',
  });
  try {
    const r = detectProduct(root);
    assert.deepEqual(r.componentsCandidates, ['components/ui', 'src/components/ui']);
    assert.deepEqual(r.stylesheets, []);
    const custom = detectProduct(root, { skip: ['design-space', 'src'] });
    assert.deepEqual(custom.componentsCandidates, ['components/ui']);
  } finally { rmSync(root, { recursive: true }); }
});

test('opts.components is listed when it exists', () => {
  const root = fixture({ 'lib/widgets/': '' });
  try {
    assert.deepEqual(detectProduct(root, { components: 'lib/widgets' }).componentsCandidates, ['lib/widgets']);
    assert.deepEqual(detectProduct(root, { components: 'nope' }).componentsCandidates, []);
  } finally { rmSync(root, { recursive: true }); }
});

test('a Tailwind v3 stylesheet with only @tailwind directives is a stylesheet', () => {
  const root = fixture({
    'package.json': pkg({ tailwindcss: '^3.4.0' }),
    'src/app.css': '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
    'src/plain.css': '.a { color: red }',
  });
  try {
    assert.deepEqual(detectProduct(root).stylesheets, ['src/app.css']);
  } finally { rmSync(root, { recursive: true }); }
});

test('a Tailwind v4 stylesheet with only @import "tailwindcss" is a stylesheet, any quotes', () => {
  const root = fixture({
    'package.json': pkg({ tailwindcss: '^4.1.0' }),
    'src/double.css': '@import "tailwindcss";\n',
    'src/single.css': "@import 'tailwindcss' source(\"../app\");\n",
    'src/other.css': '@import "./reset.css";\n',
  });
  try {
    assert.deepEqual(detectProduct(root).stylesheets, ['src/double.css', 'src/single.css']);
  } finally { rmSync(root, { recursive: true }); }
});

test('a Next.js layout without src/: app/ and styles/ stylesheets are found', () => {
  const root = fixture({
    'package.json': pkg({ tailwindcss: '^4.1.0', next: '15.0.0' }),
    'app/globals.css': '@import "tailwindcss";\n:root { --background: #fff; }\n',
    'styles/theme.css': ':root { --brand: #06c; }\n',
    'pages/_app.css': '.a { color: red }',
    'components/ui/': '',
  });
  try {
    const r = detectProduct(root);
    assert.deepEqual(r.stylesheets, ['app/globals.css', 'styles/theme.css']);
    assert.deepEqual(r.componentsCandidates, ['components/ui']);
  } finally { rmSync(root, { recursive: true }); }
});

test('build output, hidden and public folders are never scanned for stylesheets', () => {
  const root = fixture({
    '.next/static/a.css': ':root { --x: 1 }',
    'build/a.css': ':root { --x: 1 }',
    'out/a.css': ':root { --x: 1 }',
    'coverage/a.css': ':root { --x: 1 }',
    'public/vendor.css': ':root { --x: 1 }',
    'lib/tokens.css': ':root { --y: 1 }',
  });
  try {
    assert.deepEqual(detectProduct(root).stylesheets, ['lib/tokens.css']);
  } finally { rmSync(root, { recursive: true }); }
});

test('Tailwind v3 darkMode "class" gives the class mechanism when the CSS has no .dark rule', () => {
  const cases = [
    ['module.exports = { darkMode: "class" };', { kind: 'class', className: 'dark' }],
    ["module.exports = { darkMode: ['class', '.dark'] };", { kind: 'class', className: 'dark' }],
    ["export default { darkMode: 'selector' };", { kind: 'class', className: 'dark' }],
    ['module.exports = { darkMode: "media" };', { kind: 'none' }],
    ['module.exports = {};', { kind: 'none' }],
  ];
  for (const [config, expected] of cases) {
    const root = fixture({
      'package.json': pkg({ tailwindcss: '^3.4.0' }),
      'tailwind.config.js': config,
      'src/app.css': '@tailwind base;\n:root { --x: 1 }\n',
    });
    try { assert.deepEqual(detectProduct(root).theme, expected, config); }
    finally { rmSync(root, { recursive: true }); }
  }
});

test('next/font/google imports in layouts give the font families', () => {
  const root = fixture({
    'package.json': pkg({ next: '15' }),
    'app/layout.tsx': 'import { Inter, Playfair_Display } from "next/font/google";\nimport { Geist as G } from \'next/font/google\';\n',
    'app/globals.css': ':root { --a: #fff; }',
  });
  try { assert.deepEqual(detectProduct(root).nextFonts, ['Inter', 'Playfair Display', 'Geist']); } finally { rmSync(root, { recursive: true }); }
});

test('nextFonts reads src/app layouts and pages/_app, and is empty without them', () => {
  const root = fixture({
    'src/app/blog/layout.jsx': 'import { Lora } from "next/font/google";',
    'pages/_app.js': 'import { Inter } from "next/font/google";',
    'src/components/x.tsx': 'import { Nope } from "next/font/google";',
  });
  const none = fixture({ 'src/app/layout.tsx': 'export default () => null;' });
  try {
    assert.deepEqual(detectProduct(root).nextFonts, ['Lora', 'Inter']);
    assert.deepEqual(detectProduct(none).nextFonts, []);
  } finally { rmSync(root, { recursive: true }); rmSync(none, { recursive: true }); }
});

test('nextFonts ignores a homepages folder and the skipped design space', () => {
  const root = fixture({
    'src/homepages/_app.js': 'import { Inter } from "next/font/google";',
    'design-space/app/layout.tsx': 'import { Lora } from "next/font/google";',
  });
  try { assert.deepEqual(detectProduct(root).nextFonts, []); } finally { rmSync(root, { recursive: true }); }
});
