import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildNav, filterNav, productName, tokenPages, labelOf } from '../../skills/design-space/templates/app/src/shell/nav.mjs';
import { resolveRoute } from '../../skills/design-space/templates/app/src/shell/routes.mjs';

const AUDITS = [
  { name: 'accessibility', title: 'Accessibility' },
  { name: 'contrast', title: 'Contrast' },
];
const full = {
  tokenKinds: ['shadow', 'text', 'color', 'font', 'radius', 'spacing', 'breakpoint'],
  components: ['button', 'alert-dialog'],
  shared: ['stat-card'],
  flows: [{ name: 'checkout', title: 'Checkout' }],
  audits: AUDITS,
};

test('buildNav orders sections tokens, components, shared, prototypes, audits', () => {
  assert.deepEqual(buildNav(full).map((s) => s.key), ['tokens', 'components', 'shared', 'prototypes', 'audits']);
});

test('token kinds become pages; text and font share one Typography item', () => {
  const tokens = buildNav(full).find((s) => s.key === 'tokens');
  assert.deepEqual(tokens.items.map((i) => i.label), ['Colors', 'Typography', 'Spacing', 'Radius', 'Shadow', 'Other']);
  assert.deepEqual(tokens.items.map((i) => i.href), [
    '#/tokens/color', '#/tokens/typography', '#/tokens/spacing', '#/tokens/radius', '#/tokens/shadow', '#/tokens/other',
  ]);
  assert.deepEqual(tokenPages(['font']).map((p) => p.slug), ['typography']);
  assert.deepEqual(tokenPages(['text', 'font'])[0].kinds, ['font', 'text']);
});

test('components are alphabetical by label with readable labels and encoded hrefs', () => {
  const nav = buildNav({ ...full, components: ['button', 'alert-dialog', 'forms/text_input'] });
  const items = nav.find((s) => s.key === 'components').items;
  assert.deepEqual(items.map((i) => i.label), ['Alert dialog', 'Button', 'Forms / text input']);
  assert.equal(items[2].href, '#/components/forms%2Ftext_input');
  assert.equal(labelOf('alert-dialog'), 'Alert dialog');
});

test('empty sections are omitted except Prototypes, which always has All prototypes', () => {
  const nav = buildNav({ tokenKinds: [], components: [], shared: [], flows: [], audits: [] });
  assert.deepEqual(nav.map((s) => s.key), ['prototypes']);
  assert.deepEqual(nav[0].items, [{ href: '#/prototypes', label: 'All prototypes' }]);
  const withFlow = buildNav(full).find((s) => s.key === 'prototypes').items;
  assert.deepEqual(withFlow[1], { href: '#/preview/checkout', label: 'Checkout' });
  const audits = buildNav(full).find((s) => s.key === 'audits').items;
  assert.deepEqual(audits[1], { href: '#/audits/contrast', label: 'Contrast' });
});

test('filterNav keeps only matching labels, case-insensitive, over 200 items', () => {
  const nouns = ['Card', 'Dialog', 'Input', 'Select', 'Table', 'Tooltip', 'Menu', 'Badge'];
  const components = [];
  for (let i = 0; i < 200; i++) components.push(i % 10 === 0 ? `button-${i}` : `${nouns[i % nouns.length].toLowerCase()}-${i}`);
  const nav = buildNav({ ...full, components });
  const hits = filterNav(nav, 'but');
  assert.deepEqual(hits.map((s) => s.key), ['components']);
  const labels = hits[0].items.map((i) => i.label);
  assert.equal(labels.length, 20);
  assert.ok(labels.every((l) => l.startsWith('Button')));
  assert.deepEqual(filterNav(nav, 'BUT'), hits);
  assert.equal(filterNav(nav, ''), nav);
  assert.equal(filterNav(nav, '  ').length, nav.length);
  assert.deepEqual(filterNav(nav, 'zzz'), []);
});

test('resolveRoute maps the routes', () => {
  assert.deepEqual(resolveRoute('/'), { page: 'home', params: {} });
  assert.deepEqual(resolveRoute(''), { page: 'home', params: {} });
  assert.deepEqual(resolveRoute('/tokens/color'), { page: 'tokens', params: { kind: 'color' } });
  assert.deepEqual(resolveRoute('/components/button'), { page: 'component', params: { name: 'button' } });
  assert.deepEqual(resolveRoute('/components/forms%2Finput'), { page: 'component', params: { name: 'forms/input' } });
  assert.deepEqual(resolveRoute('/shared/stat-card'), { page: 'shared', params: { name: 'stat-card' } });
  assert.deepEqual(resolveRoute('/prototypes'), { page: 'prototypes', params: {} });
  assert.deepEqual(resolveRoute('/preview/checkout'), { page: 'preview', params: { flow: 'checkout', page: '' } });
  assert.deepEqual(resolveRoute('/preview/checkout/cart/2'), { page: 'preview', params: { flow: 'checkout', page: '/cart/2' } });
  assert.deepEqual(resolveRoute('/audits'), { page: 'audits', params: {} });
  assert.deepEqual(resolveRoute('/audits/contrast'), { page: 'audit', params: { name: 'contrast' } });
});

test('resolveRoute redirects the old routes', () => {
  assert.deepEqual(resolveRoute('/design-system'), { redirect: '/tokens/color' });
  assert.deepEqual(resolveRoute('/design-system/audits'), { redirect: '/audits' });
  assert.deepEqual(resolveRoute('/design-system/audits/contrast'), { redirect: '/audits/contrast' });
  assert.deepEqual(resolveRoute('/design-system/specimens/button'), { redirect: '/components/button' });
  assert.deepEqual(resolveRoute('/design-system/specimens/forms/input'), { redirect: '/components/forms%2Finput' });
});

test('resolveRoute: unknown paths are notFound', () => {
  for (const path of ['/nope', '/tokens', '/tokens/a/b', '/components', '/audits/a/b', '/prototypes/x', '/design-system/x']) {
    assert.equal(resolveRoute(path).page, 'notFound', path);
  }
});

test('productName: profile name, else the package name made readable', () => {
  assert.equal(productName({ name: 'Acme Studio', packageName: 'web-app' }), 'Acme Studio');
  assert.equal(productName({ name: null, packageName: 'web-app' }), 'Web app');
  assert.equal(productName({ name: null, packageName: '@acme/web_app' }), 'Web app');
  assert.equal(productName({ name: '  ', packageName: null }), 'Design space');
  assert.equal(productName({}), 'Design space');
});
