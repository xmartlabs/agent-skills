import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  parseRgbString, relativeLuminance, contrastRatio, wcagGrade, simulateCvd,
  deltaE2000, deltaE2000Lab, decideColor, cvdFindings, CVD_MIN_DELTA_E,
} from '../../skills/design-space/templates/app/src/audits/color.mjs';

const black = { r: 0, g: 0, b: 0 };
const white = { r: 255, g: 255, b: 255 };
const near = (a, b, eps) => assert.ok(Math.abs(a - b) <= eps, `${a} vs ${b}`);

test('contrast of black on white is 21', () => {
  near(contrastRatio(black, white), 21, 0.01);
  near(contrastRatio(white, black), 21, 0.01);
  near(relativeLuminance(white), 1, 1e-9);
});

test('mid gray #777 on white is about 4.48', () => {
  near(contrastRatio({ r: 119, g: 119, b: 119 }, white), 4.48, 0.01);
});

test('wcagGrade applies WCAG 2.1 thresholds', () => {
  assert.equal(wcagGrade(4.48, false), 'fail');
  assert.equal(wcagGrade(4.48, true), 'AA');
  assert.equal(wcagGrade(4.5, false), 'AA');
  assert.equal(wcagGrade(7, false), 'AAA');
  assert.equal(wcagGrade(4.5, true), 'AAA');
  assert.equal(wcagGrade(2.9, true), 'fail');
});

test('deltaE2000 matches the Sharma reference pair', () => {
  near(deltaE2000Lab({ L: 50, a: 2.6772, b: -79.7751 }, { L: 50, a: 0, b: -82.7485 }), 2.0425, 0.0001);
});

test('deltaE2000 of identical colors is 0 and black/white is large', () => {
  near(deltaE2000(black, black), 0, 1e-9);
  assert.ok(deltaE2000(black, white) > 90);
});

test('simulateCvd keeps grays gray', () => {
  for (const type of ['protanopia', 'deuteranopia', 'tritanopia']) {
    for (const v of [0, 64, 128, 200, 255]) {
      const o = simulateCvd({ r: v, g: v, b: v }, type);
      near(o.r, v, 1); near(o.g, v, 1); near(o.b, v, 1);
    }
  }
});

test('simulateCvd rejects an unknown type', () => {
  assert.throws(() => simulateCvd(black, 'nope'));
});

test('cvdFindings reports red/green under deuteranopia, never black/white', () => {
  const red = { r: 0xd3, g: 0x2f, b: 0x2f };
  const green = { r: 0x38, g: 0x8e, b: 0x3c };
  const out = cvdFindings([
    { a: 'danger', b: 'success', colorA: red, colorB: green },
    { a: 'ink', b: 'paper', colorA: black, colorB: white },
  ]);
  assert.ok(out.some((f) => f.a === 'danger' && f.b === 'success' && f.type === 'deuteranopia'));
  assert.ok(out.every((f) => f.deltaE < CVD_MIN_DELTA_E));
  assert.ok(!out.some((f) => f.a === 'ink'));
  for (let i = 1; i < out.length; i++) assert.ok(out[i - 1].deltaE <= out[i].deltaE);
});

test('parseRgbString handles comma and space syntax', () => {
  assert.deepEqual(parseRgbString('rgb(1 2 3 / 50%)'), { r: 1, g: 2, b: 3 });
  assert.deepEqual(parseRgbString('rgba(10, 20, 30, 0.5)'), { r: 10, g: 20, b: 30 });
  assert.deepEqual(parseRgbString('rgb(255, 0, 0)'), { r: 255, g: 0, b: 0 });
  assert.equal(parseRgbString('nope'), null);
  assert.equal(parseRgbString(''), null);
});

test('deltaE2000 matches further Sharma pairs (hue-wrap branches)', () => {
  const d = (p, q) => deltaE2000Lab({ L: p[0], a: p[1], b: p[2] }, { L: q[0], a: q[1], b: q[2] });
  near(d([50, 0, 0], [50, -1, 2]), 2.3669, 0.0001);
  near(d([50, 2.5, 0], [50, 0, -2.5]), 4.3065, 0.0001);
  near(d([50, 2.5, 0], [73, 25, -18]), 27.1492, 0.0001);
});

test('decideColor returns null unless both probes agree and the canvas accepted it', () => {
  const ok = 'rgb(10, 20, 30)';
  assert.equal(decideColor([ok, ok], ['#0a141e', '#0a141e']), ok);
  // invalid var(): each probe fell back to its inherited color
  assert.equal(decideColor(['rgb(1, 2, 3)', 'rgb(4, 5, 6)'], ['#0a141e', '#0a141e']), null);
  // canvas rejected the value: fillStyle kept each sentinel
  assert.equal(decideColor([ok, ok], ['#010203', '#040506']), null);
  assert.equal(decideColor(['', ''], ['#010203', '#010203']), null);
});
