import { test } from 'node:test';
import assert from 'node:assert/strict';
import { listComponentFiles, coverageRows } from '../../skills/design-space/templates/app/build/coverage.mjs';

const UI = 'src/components/ui/';
const base = { components: [UI + 'button.tsx', UI + 'badge.tsx'], specimenImports: new Map(), barrels: new Map(), excluded: {} };

test('listComponentFiles drops barrels, stories, tests, specs, declarations and other extensions', () => {
  const files = ['button.tsx', 'badge.tsx', 'index.ts', 'button.stories.tsx', 'types.d.ts',
    'a.test.ts', 'b.spec.jsx', 'styles.css', 'nested/index.js', 'nested/card.jsx'];
  assert.deepEqual(listComponentFiles(files), ['button.tsx', 'badge.tsx', 'nested/card.jsx']);
});

test('listComponentFiles counts only .tsx and .jsx files: types, hooks and helpers are not components', () => {
  const files = ['button.tsx', 'types.ts', 'button.types.ts', 'use-toast.ts', 'utils.js', 'card.jsx'];
  assert.deepEqual(listComponentFiles(files), ['button.tsx', 'card.jsx']);
});

test('a specimen importing the component file covers it', () => {
  const rows = coverageRows({ ...base, specimenImports: new Map([['specimens/button.tsx', [{ file: UI + 'button.tsx', names: ['Button'] }]]]) });
  assert.deepEqual(rows, [
    { file: UI + 'badge.tsx', status: 'missing' },
    { file: UI + 'button.tsx', status: 'covered', by: 'specimens/button.tsx' },
  ]);
});

const BARREL = UI + 'index.ts';
const barrelBase = {
  ...base,
  barrels: new Map([[BARREL, [
    { file: UI + 'badge.tsx', names: ['Badge'] },
    { file: UI + 'button.tsx', names: ['Button'] },
  ]]]),
};
const via = (names) => new Map([['specimens/all.tsx', [{ file: BARREL, names }]]]);
const statuses = (rows) => Object.fromEntries(rows.map((r) => [r.file.replace(UI, ''), r.status]));

test('a named import from a barrel covers only the file that export comes from', () => {
  const rows = coverageRows({ ...barrelBase, specimenImports: via(['Badge']) });
  assert.deepEqual(statuses(rows), { 'badge.tsx': 'covered', 'button.tsx': 'missing' });
  assert.equal(rows[0].by, 'specimens/all.tsx');
});

test('a namespace import of a barrel covers everything it reaches', () => {
  const rows = coverageRows({ ...barrelBase, specimenImports: via('*') });
  assert.deepEqual(statuses(rows), { 'badge.tsx': 'covered', 'button.tsx': 'covered' });
});

test('an unknown name covers nothing', () => {
  const rows = coverageRows({ ...barrelBase, specimenImports: via(['Nope']) });
  assert.deepEqual(statuses(rows), { 'badge.tsx': 'missing', 'button.tsx': 'missing' });
});

test('nested barrels are followed and cycles terminate', () => {
  const rows = coverageRows({
    ...base,
    specimenImports: via(['Badge']),
    barrels: new Map([
      [BARREL, [{ file: UI + 'more/index.ts', names: '*' }]],
      [UI + 'more/index.ts', [{ file: BARREL, names: '*' }, { file: UI + 'badge.tsx', names: ['Badge'] }]],
    ]),
  });
  assert.deepEqual(statuses(rows), { 'badge.tsx': 'covered', 'button.tsx': 'missing' });
});

test('export * resolves names through exportsOf', () => {
  const rows = coverageRows({
    ...base,
    specimenImports: via(['Badge']),
    barrels: new Map([[BARREL, [{ file: UI + 'badge.tsx', names: '*' }, { file: UI + 'button.tsx', names: '*' }]]]),
    exportsOf: new Map([[UI + 'badge.tsx', ['Badge']], [UI + 'button.tsx', ['Button']]]),
  });
  assert.deepEqual(statuses(rows), { 'badge.tsx': 'covered', 'button.tsx': 'missing' });
});

test('an excluded entry with a reason is excluded', () => {
  const rows = coverageRows({ ...base, excluded: { [UI + 'badge.tsx']: 'internal helper' } });
  assert.deepEqual(rows[0], { file: UI + 'badge.tsx', status: 'excluded', reason: 'internal helper' });
});

test('an excluded entry with an empty reason is missing', () => {
  const rows = coverageRows({ ...base, excluded: { [UI + 'badge.tsx']: '  ' } });
  assert.equal(rows[0].status, 'missing');
});

test('a specimen wins over an exclusion', () => {
  const rows = coverageRows({
    ...base,
    excluded: { [UI + 'button.tsx']: 'why' },
    specimenImports: new Map([['specimens/button.tsx', [{ file: UI + 'button.tsx', names: ['Button'] }]]]),
  });
  assert.equal(rows.find((r) => r.file === UI + 'button.tsx').status, 'covered');
});

test('rows are sorted by file', () => {
  const rows = coverageRows({ ...base, components: ['z.tsx', 'a.tsx', 'm.tsx'] });
  assert.deepEqual(rows.map((r) => r.file), ['a.tsx', 'm.tsx', 'z.tsx']);
});

test('imports of a helper in specimens/_shared/ do not count', async () => {
  const { isSpecimenFile } = await import('../../skills/design-space/templates/app/build/coverage.mjs');
  assert.equal(isSpecimenFile('specimens/button.tsx'), true);
  assert.equal(isSpecimenFile('specimens/forms/Field.tsx'), true);
  assert.equal(isSpecimenFile('specimens/_shared/Section.tsx'), false);
  assert.equal(isSpecimenFile('specimens/_shared/deep/Row.tsx'), false);
  const rows = coverageRows({
    components: ['ui/badge.tsx'],
    specimenImports: new Map([['specimens/_shared/Section.tsx', [{ file: 'ui/badge.tsx', names: ['Badge'] }]]]),
    barrels: new Map(),
    excluded: {},
  });
  assert.deepEqual(rows, [{ file: 'ui/badge.tsx', status: 'missing' }]);
});
