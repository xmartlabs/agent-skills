import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { catalogPlugin } from '../../skills/design-space/templates/app/build/catalog-plugin.mjs';

const VIRTUAL = 'virtual:design-space/catalog';
const UI = 'src/components/ui/';

function repo(files, profile = { paths: { components: UI } }) {
  const root = mkdtempSync(join(tmpdir(), 'catalog-'));
  const all = { '.design-space.json': JSON.stringify(profile), ...files };
  for (const [path, text] of Object.entries(all)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return { root, appRoot: join(root, 'ds'), done: () => rmSync(root, { recursive: true, force: true }) };
}

// A fake Vite context: "@/" is an alias for src/, extensions are probed in order, packages do not resolve.
function context(root, existing) {
  return {
    addWatchFile() {},
    async resolve(spec, from) {
      let path = spec.startsWith('@/') ? join(root, 'src', spec.slice(2)) : spec.startsWith('.') ? resolve(dirname(from), spec) : null;
      if (!path) return null;
      for (const candidate of [path, path + '.tsx', path + '.ts', join(path, 'index.ts')]) {
        if (existing.has(candidate)) return { id: candidate };
      }
      return null;
    },
  };
}

async function catalog(files, profile) {
  const r = repo(files, profile);
  try {
    const existing = new Set(Object.keys(files).map((f) => join(r.root, f)));
    const plugin = catalogPlugin({ root: r.root, appRoot: r.appRoot });
    const code = await plugin.load.call(context(r.root, existing), plugin.resolveId(VIRTUAL));
    const rows = JSON.parse(code.match(/components = (.*);\n/)[1]);
    return {
      rows,
      folder: JSON.parse(code.match(/componentsFolder = (.*);\n/)[1]),
      meta: JSON.parse(code.match(/meta = (.*);\n/)[1]),
    };
  } finally {
    r.done();
  }
}

const component = {
  [UI + 'badge.tsx']: 'export function Badge() { return null; }',
  [UI + 'button.tsx']: 'export function Button() { return null; }',
  [UI + 'index.ts']: 'export { Badge } from "./badge";\nexport { Button } from "./button";\n',
};

test('resolves the virtual module id', () => {
  const plugin = catalogPlugin({ root: '/nowhere', appRoot: '/nowhere/ds' });
  assert.equal(plugin.resolveId(VIRTUAL), '\0' + VIRTUAL);
  assert.equal(plugin.resolveId('react'), null);
});

test('without paths.components the catalog is empty', async () => {
  const { rows, folder } = await catalog({ ...component }, {});
  assert.deepEqual(rows, []);
  assert.equal(folder, null);
});

test('a named import through an aliased barrel covers only that component', async () => {
  const { rows, folder } = await catalog({
    ...component,
    'ds/specimens/badge.tsx': 'import { Badge } from "@/components/ui";\nexport default () => <Badge />;',
  }, { paths: { components: './src/components/ui//' } });
  assert.equal(folder, 'src/components/ui');
  assert.deepEqual(rows, [
    { file: UI + 'badge.tsx', status: 'covered', by: 'specimens/badge.tsx' },
    { file: UI + 'button.tsx', status: 'missing' },
  ]);
});

test('a direct aliased import of a component file covers it; .ts specimens count', async () => {
  const { rows } = await catalog({
    ...component,
    'ds/specimens/b.ts': 'import { Button } from "@/components/ui/button";',
  });
  assert.equal(rows.find((r) => r.file === UI + 'button.tsx').status, 'covered');
  assert.equal(rows.find((r) => r.file === UI + 'badge.tsx').status, 'missing');
});

test('index test and story files are not barrels and not components', async () => {
  const { rows } = await catalog({ ...component, [UI + 'index.test.ts']: 'export * from "./badge";' });
  assert.deepEqual(rows.map((r) => r.file), [UI + 'badge.tsx', UI + 'button.tsx']);
});

test('an excluded component with a reason is excluded', async () => {
  const { rows } = await catalog({
    ...component,
    'ds/audits/coverage-excluded.json': JSON.stringify({ [UI + 'badge.tsx']: 'internal' }),
  });
  assert.deepEqual(rows[0], { file: UI + 'badge.tsx', status: 'excluded', reason: 'internal' });
});

test('invalid JSON in coverage-excluded.json is an error naming the file', async () => {
  await assert.rejects(
    catalog({ ...component, 'ds/audits/coverage-excluded.json': '{nope' }),
    /coverage-excluded\.json is not valid JSON/,
  );
});

test('coverage-excluded.json that is not an object is an error naming the file', async () => {
  await assert.rejects(
    catalog({ ...component, 'ds/audits/coverage-excluded.json': '[]' }),
    /coverage-excluded\.json has the wrong shape/,
  );
});

test('handleHotUpdate keeps the modules Vite found and adds the catalog', () => {
  const r = repo({});
  const plugin = catalogPlugin({ root: r.root, appRoot: r.appRoot });
  const catalogModule = { id: '\0' + VIRTUAL };
  const own = { id: 'specimen' };
  const invalidated = [];
  const server = {
    moduleGraph: {
      getModuleById: (id) => (id === '\0' + VIRTUAL ? catalogModule : undefined),
      invalidateModule: (m) => invalidated.push(m),
    },
  };
  const update = (file, modules = [own]) => plugin.handleHotUpdate({ file, server, modules });
  assert.deepEqual(update(join(r.appRoot, 'specimens/a.tsx')), [own, catalogModule]);
  assert.deepEqual(update(join(r.appRoot, 'audits/coverage-excluded.json'), [catalogModule]), [catalogModule]);
  assert.deepEqual(update(join(r.root, UI, 'button.tsx')), [own, catalogModule]);
  assert.equal(update(join(r.root, 'src/other.ts')), undefined);
  assert.equal(invalidated.length, 3);
  r.done();
});

test('meta carries the profile name and the package name; no git gives no branch', async () => {
  const { meta } = await catalog({ 'package.json': '{"name":"@acme/web-app"}' }, { name: 'Acme Studio' });
  assert.deepEqual(meta, { name: 'Acme Studio', branch: null, packageName: '@acme/web-app' });
  const bare = await catalog({}, {});
  assert.deepEqual(bare.meta, { name: null, branch: null, packageName: null });
});

test('meta.branch is the checked-out git branch at build time', async () => {
  const r = repo({ 'package.json': '{"name":"web-app"}' }, {});
  try {
    const git = (...args) => execFileSync('git', ['-C', r.root, ...args], { stdio: 'ignore' });
    git('init', '-q');
    git('checkout', '-q', '-b', 'feature/sidebar');
    git('-c', 'user.name=dev', '-c', 'user.email=dev@example.com', 'commit', '-q', '--allow-empty', '-m', 'init');
    const plugin = catalogPlugin({ root: r.root, appRoot: r.appRoot });
    const code = await plugin.load.call(context(r.root, new Set()), plugin.resolveId(VIRTUAL));
    assert.equal(JSON.parse(code.match(/meta = (.*);\n/)[1]).branch, 'feature/sidebar');
  } finally {
    r.done();
  }
});

test('a helper in specimens/_shared/ is not a specimen and covers nothing', async () => {
  const { rows } = await catalog({
    ...component,
    'ds/specimens/_shared/Section.tsx': 'import { Badge } from "@/components/ui/badge";\nexport function Section() { return <Badge />; }',
    'ds/specimens/button.tsx': 'import { Section } from "./_shared/Section";\nimport { Button } from "@/components/ui/button";\nexport default () => <Section><Button /></Section>;',
  });
  assert.deepEqual(rows, [
    { file: UI + 'badge.tsx', status: 'missing' },
    { file: UI + 'button.tsx', status: 'covered', by: 'specimens/button.tsx' },
  ]);
});
