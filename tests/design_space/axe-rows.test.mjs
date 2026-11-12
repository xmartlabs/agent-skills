import { test } from 'node:test';
import assert from 'node:assert/strict';
import { axeRows } from '../../skills/design-space/templates/app/src/audits/axe-rows.mjs';
import { specimenName } from '../../skills/design-space/templates/app/src/shell/specimen-name.mjs';

test('axeRows makes one row per failing node', () => {
  const rows = axeRows('/design-system/specimens/button', [
    { id: 'button-name', impact: 'critical', nodes: [{ html: '<button></button>' }, { html: '<button class="x"></button>' }] },
    { id: 'image-alt', impact: null, nodes: [{ html: '<img>' }] },
  ]);
  assert.deepEqual(rows, [
    { target: '/design-system/specimens/button', rule: 'button-name', impact: 'critical', html: '<button></button>' },
    { target: '/design-system/specimens/button', rule: 'button-name', impact: 'critical', html: '<button class="x"></button>' },
    { target: '/design-system/specimens/button', rule: 'image-alt', impact: 'unknown', html: '<img>' },
  ]);
});

test('axeRows is empty without violations', () => {
  assert.deepEqual(axeRows('/x', []), []);
});

test('specimenName drops the folder prefix and the extension', () => {
  assert.equal(specimenName('../../specimens/forms/Field.tsx'), 'forms/Field');
  assert.equal(specimenName('../../specimens/Button.tsx'), 'Button');
});
