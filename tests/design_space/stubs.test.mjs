import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { stubPlugin } from '../../skills/design-space/templates/app/stubs/plugin.mjs';

const REPO = '/work/acme/web-app';
const APP = `${REPO}/design-space`;
const SOCKET = `${REPO}/src/networking/socket.ts`;
const STUB = `${APP}/stubs/socket.ts`;

// A stand-in for the Vite resolver: relative and absolute paths resolve on disk,
// "@/" is an alias for the repository's src folder, anything else is unresolved.
function fakeContext() {
  return {
    async resolve(source, importer) {
      if (source.startsWith('.')) return { id: resolve(importer.replace(/[^/]+$/, ''), source) };
      if (source.startsWith('/')) return { id: source };
      if (source.startsWith('@/')) return { id: `${REPO}/src/${source.slice(2)}` };
      return null;
    },
  };
}

const plugin = stubPlugin(
  [
    { match: 'src/networking/socket.ts', stub: 'stubs/socket.ts' },
    { match: 'next/link', stub: 'stubs/next/link.tsx' },
  ],
  { root: REPO, appRoot: APP },
);
const run = (source, importer = `${REPO}/src/pages/index.tsx`) =>
  plugin.resolveId.call(fakeContext(), source, importer, {});

test('runs before the other resolvers', () => {
  assert.equal(plugin.enforce, 'pre');
});

test('stubs a relative import of the matched file', async () => {
  assert.equal(await run('../networking/socket.ts'), STUB);
  assert.equal(await run('../networking/socket.ts?v=1'), STUB);
});

test('stubs an absolute import of the matched file', async () => {
  assert.equal(await run(SOCKET), STUB);
});

test('stubs an aliased import that resolves to the matched file', async () => {
  assert.equal(await run('@/networking/socket.ts'), STUB);
});

test('leaves other files alone', async () => {
  assert.equal(await run('../widgets.ts'), null);
  assert.equal(await run('@/widgets.ts'), null);
  assert.equal(await run('react'), null);
});

test('a bare package name matches the bare specifier, even when it is not installed', async () => {
  assert.equal(await run('next/link'), `${APP}/stubs/next/link.tsx`);
  assert.equal(await run('next/image'), null);
});

test('does not redirect imports made by the stub itself', async () => {
  assert.equal(await run(SOCKET, STUB), null);
});

// --- flowHref: product paths become routes inside the flow being previewed ---
const { flowHref } = await import('../../skills/design-space/templates/app/stubs/next/flow-href.ts');

test('flowHref puts a product path under the flow and keeps the toolbar choices', () => {
  assert.equal(flowHref('/orders', '#/preview/checkout?state=empty'), '#/preview/checkout/orders?state=empty');
  assert.equal(
    flowHref('/orders/1', '#/preview/checkout/cart?state=error&role=admin&device=mobile&x=1'),
    '#/preview/checkout/orders/1?state=error&role=admin&device=mobile',
  );
  assert.equal(flowHref('/', '#/preview/checkout/cart'), '#/preview/checkout');
});

test('flowHref lets the path set its own query', () => {
  assert.equal(flowHref('/orders?state=loading', '#/preview/checkout?state=empty'), '#/preview/checkout/orders?state=loading');
});

test('flowHref leaves absolute URLs and relative paths alone', () => {
  const hash = '#/preview/checkout?state=empty';
  assert.equal(flowHref('https://example.test/a', hash), 'https://example.test/a');
  assert.equal(flowHref('//example.test/a', hash), '//example.test/a');
  assert.equal(flowHref('orders', hash), 'orders');
  assert.equal(flowHref('#top', hash), '#top');
});

test('flowHref outside a flow keeps the plain hash route', () => {
  assert.equal(flowHref('/orders', '#/'), '#/orders');
  assert.equal(flowHref('/orders', ''), '#/orders');
});
