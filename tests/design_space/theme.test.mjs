import { test } from 'node:test';
import assert from 'node:assert/strict';
import { applyTheme, canSwitch, storedTheme, saveTheme, syncThemeAcrossFrames, THEME_KEY, THEME_EVENT } from '../../skills/design-space/templates/app/src/shell/theme.mjs';
import { auditName } from '../../skills/design-space/templates/app/src/shell/audit-route.mjs';

const fakeRoot = () => {
  const calls = [];
  return {
    calls,
    classList: { add: (c) => calls.push(['add', c]), remove: (c) => calls.push(['remove', c]) },
    setAttribute: (n, v) => calls.push(['set', n, v]),
    removeAttribute: (n) => calls.push(['unset', n]),
  };
};
const fakeStorage = (initial = {}) => {
  const data = { ...initial };
  return { data, getItem: (k) => (k in data ? data[k] : null), setItem: (k, v) => { data[k] = v; } };
};
const cls = { kind: 'class', className: 'dark' };
const attr = { kind: 'attribute', attribute: 'data-theme', dark: 'dark', light: 'light' };

test('class mechanism adds and removes the dark class', () => {
  const dark = fakeRoot();
  applyTheme(dark, cls, 'dark');
  assert.deepEqual(dark.calls, [['add', 'dark']]);
  const light = fakeRoot();
  applyTheme(light, cls, 'light');
  assert.deepEqual(light.calls, [['remove', 'dark']]);
});

test('attribute mechanism sets the attribute to the dark or light value', () => {
  const dark = fakeRoot();
  applyTheme(dark, attr, 'dark');
  assert.deepEqual(dark.calls, [['set', 'data-theme', 'dark']]);
  const light = fakeRoot();
  applyTheme(light, { ...attr, attribute: 'data-mode', light: 'day' }, 'light');
  assert.deepEqual(light.calls, [['set', 'data-mode', 'day']]);
});

test('media and none mechanisms touch nothing', () => {
  for (const mechanism of [{ kind: 'media' }, { kind: 'none' }]) {
    const root = fakeRoot();
    applyTheme(root, mechanism, 'dark');
    assert.deepEqual(root.calls, []);
  }
});

test('only class and attribute can switch', () => {
  assert.equal(canSwitch(cls), true);
  assert.equal(canSwitch(attr), true);
  assert.equal(canSwitch({ kind: 'media' }), false);
  assert.equal(canSwitch({ kind: 'none' }), false);
});

test('stored theme defaults to light and ignores junk', () => {
  assert.equal(THEME_KEY, 'design-space:theme');
  assert.equal(storedTheme(fakeStorage(), cls), 'light');
  assert.equal(storedTheme(fakeStorage({ [THEME_KEY]: 'dark' }), cls), 'dark');
  assert.equal(storedTheme(fakeStorage({ [THEME_KEY]: 'purple' }), cls), 'light');
});

test('a single-theme product never reads or writes storage', () => {
  for (const mechanism of [{ kind: 'media' }, { kind: 'none' }]) {
    const storage = fakeStorage({ [THEME_KEY]: 'dark' });
    assert.equal(storedTheme(storage, mechanism), 'light');
    saveTheme(storage, mechanism, 'dark');
    assert.deepEqual(storage.data, { [THEME_KEY]: 'dark' });
  }
  const storage = fakeStorage();
  saveTheme(storage, cls, 'dark');
  assert.equal(storage.data[THEME_KEY], 'dark');
});

test('storage failures do not throw', () => {
  const broken = { getItem() { throw new Error('denied'); }, setItem() { throw new Error('denied'); } };
  assert.equal(storedTheme(broken, cls), 'light');
  assert.doesNotThrow(() => saveTheme(broken, cls, 'dark'));
});

test('auditName reads the audit from the path', () => {
  assert.equal(auditName('/audits/contrast'), 'contrast');
  assert.equal(auditName('/audits'), null);
  assert.equal(auditName('/audits/'), null);
  assert.equal(auditName('/tokens/color'), null);
  assert.equal(auditName('/audits/a/b'), null);
});

const fakeWindow = (storage) => {
  const listeners = {};
  const dispatched = [];
  const root = fakeRoot();
  return {
    root, dispatched, listeners,
    win: {
      localStorage: storage,
      document: { documentElement: root },
      addEventListener: (type, fn) => { listeners[type] = fn; },
      dispatchEvent: (event) => dispatched.push(event.type),
    },
  };
};

test('a storage event for the theme key applies the theme and announces it', () => {
  const f = fakeWindow(fakeStorage({ [THEME_KEY]: 'dark' }));
  syncThemeAcrossFrames(f.win, cls);
  f.listeners.storage({ key: THEME_KEY });
  assert.deepEqual(f.root.calls, [['add', 'dark']]);
  assert.deepEqual(f.dispatched, [THEME_EVENT]);
});

test('storage events for other keys are ignored', () => {
  const f = fakeWindow(fakeStorage({ [THEME_KEY]: 'dark' }));
  syncThemeAcrossFrames(f.win, cls);
  f.listeners.storage({ key: 'other' });
  assert.deepEqual(f.root.calls, []);
  assert.deepEqual(f.dispatched, []);
});

test('a single-theme mechanism installs no listener', () => {
  const f = fakeWindow(fakeStorage({ [THEME_KEY]: 'dark' }));
  syncThemeAcrossFrames(f.win, { kind: 'media' });
  assert.equal(f.listeners.storage, undefined);
});
