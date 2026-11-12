import { test } from 'node:test';
import assert from 'node:assert/strict';
import { snippetFor } from '../../skills/design-space/templates/app/src/components/snippet.mjs';

const defaults = { variant: 'default', size: 'md', disabled: false };

test('props equal to their control default are omitted; children sit on their own line', () => {
  assert.equal(
    snippetFor('Button', { variant: 'default', size: 'md', disabled: false }, 'Button', defaults),
    '<Button>\n  Button\n</Button>',
  );
});

test('a changed string is x="v"; a true boolean is a bare x', () => {
  assert.equal(
    snippetFor('Button', { variant: 'primary', size: 'md', disabled: true }, 'Button', defaults),
    '<Button variant="primary" disabled>\n  Button\n</Button>',
  );
});

test('without children the tag closes itself', () => {
  assert.equal(snippetFor('Badge', { variant: 'success' }), '<Badge variant="success" />');
  assert.equal(snippetFor('Badge', { variant: 'success' }, ''), '<Badge variant="success" />');
  assert.equal(snippetFor('Badge', {}), '<Badge />');
});

test('numbers are x={n}; a false boolean is omitted unless its default is true', () => {
  assert.equal(snippetFor('Rating', { value: 3, readOnly: false }), '<Rating value={3} />');
  assert.equal(snippetFor('Switch', { checked: false }, undefined, { checked: true }), '<Switch checked={false} />');
});

test('text that JSX cannot hold literally is quoted as an expression', () => {
  assert.equal(snippetFor('Input', { placeholder: 'Say "hi"' }), '<Input placeholder={"Say \\"hi\\""} />');
  assert.equal(snippetFor('Tag', {}, 'a < b {c}'), '<Tag>\n  {"a < b {c}"}\n</Tag>');
});

test('undefined props are omitted', () => {
  assert.equal(snippetFor('Badge', { variant: undefined }), '<Badge />');
});

test('the tag is the displayName, else the page name in PascalCase', async () => {
  const { tagNameOf } = await import('../../skills/design-space/templates/app/src/components/snippet.mjs');
  assert.equal(tagNameOf('Button', 'whatever'), 'Button');
  assert.equal(tagNameOf(undefined, 'alert-dialog'), 'AlertDialog');
  assert.equal(tagNameOf('', 'forms/text_input'), 'TextInput');
  assert.equal(tagNameOf(undefined, 'Badge'), 'Badge');
  assert.equal(tagNameOf(undefined, '---'), 'Component');
});
