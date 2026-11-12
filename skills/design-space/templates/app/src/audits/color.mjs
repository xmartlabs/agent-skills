// Color math for the audit pages: WCAG contrast, color-vision simulation, CIEDE2000.
// Pure functions, no Node imports: node --test and the browser bundle both load this file.
// Only readColor touches the DOM, and only when it is called.

/** @typedef {{ r: number, g: number, b: number }} RGB  channels 0-255 */

const clamp01 = (v) => Math.min(1, Math.max(0, v));
const toLinear = (c) => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
};
const fromLinear = (v) => {
  const x = clamp01(v);
  return 255 * (x <= 0.0031308 ? x * 12.92 : 1.055 * x ** (1 / 2.4) - 0.055);
};

/** Parse "rgb(r g b / a)" or "rgba(r, g, b, a)" into 0-255 channels, or null. */
export function parseRgbString(s) {
  const m = /^\s*rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)\s*(?:[,/]\s*[\d.]+%?\s*)?\)\s*$/i.exec(String(s ?? ''));
  if (!m) return null;
  const [r, g, b] = [m[1], m[2], m[3]].map(Number);
  if (![r, g, b].every((v) => Number.isFinite(v))) return null;
  return { r, g, b };
}

// Inherited colors the probe sits under, one per probe. A real color ignores them; an
// invalid var() falls back to the inherited color, so the two probes disagree.
const PROBE_INHERIT = ['rgb(1, 2, 3)', 'rgb(4, 5, 6)'];
// Canvas fillStyle values set before assigning the probed color. A rejected assignment
// leaves the previous value, so the two leftovers differ.
const FILL_SENTINELS = ['#010203', '#040506'];

/**
 * Pure decision behind readColor. `probes` are the computed colors of the two probes (one
 * per PROBE_INHERIT), `fills` the canvas fillStyle read back after assigning each probed
 * color over FILL_SENTINELS. Returns the color string to paint, or null when the variable
 * is not a color: empty, a probe fell back to its inherited color, or the canvas rejected it.
 */
export function decideColor(probes, fills) {
  const [p1, p2] = probes;
  if (!p1 || !p2 || p1 !== p2) return null;
  const [f1, f2] = fills;
  if (f1 !== f2) return null;
  return p1;
}

let pixelCtx = null;
const getPixelCtx = () => {
  if (!pixelCtx) {
    const canvas = document.createElement('canvas');
    canvas.width = 1;
    canvas.height = 1;
    pixelCtx = canvas.getContext('2d', { willReadFrequently: true });
  }
  return pixelCtx;
};

// Shared probe/canvas path: `paint(probe)` sets the color under test, `raw` reads what it was set from.
function resolveColor(paint, raw, scope) {
  const ctx = getPixelCtx();
  if (!ctx) return null;
  const probes = [];
  let rawValue = '';
  for (const inherit of PROBE_INHERIT) {
    const wrap = document.createElement('div');
    wrap.style.display = 'none';
    wrap.style.color = inherit;
    const probe = document.createElement('span');
    paint(probe);
    wrap.appendChild(probe);
    scope.appendChild(wrap);
    const cs = getComputedStyle(probe);
    probes.push(cs.color);
    rawValue = raw(cs);
    wrap.remove();
  }
  if (!rawValue) return null;
  const fills = FILL_SENTINELS.map((sentinel, i) => {
    ctx.fillStyle = sentinel;
    ctx.fillStyle = probes[i];
    return ctx.fillStyle;
  });
  const color = decideColor(probes, fills);
  if (!color) return null;
  ctx.fillStyle = color;
  ctx.clearRect(0, 0, 1, 1);
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = ctx.getImageData(0, 0, 1, 1).data;
  if (a === 0) return parseRgbString(ctx.fillStyle);
  return { r, g, b };
}

/**
 * Browser only. Resolve a CSS custom property to sRGB. Probe elements inside `scope` take
 * `color: var(<cssVar>)`; the computed color is painted on a 1x1 canvas and the pixel is
 * read back, so oklch(), color() and hsl() values all end up as sRGB. Returns null when
 * the variable is empty or does not hold a color.
 */
export function readColor(cssVar, scope = document.body) {
  return resolveColor(
    (probe) => { probe.style.color = `var(${cssVar})`; },
    (cs) => cs.getPropertyValue(cssVar).trim(),
    scope,
  );
}

/** Browser only. Same as readColor for a literal CSS color (a token with no custom property). Null when it is not a color. */
export function readColorValue(value, scope = document.body) {
  const literal = String(value ?? '').trim();
  if (!literal) return null;
  return resolveColor((probe) => { probe.style.color = literal; }, () => literal, scope);
}

/** WCAG 2.1 relative luminance. */
export function relativeLuminance(c) {
  return 0.2126 * toLinear(c.r) + 0.7152 * toLinear(c.g) + 0.0722 * toLinear(c.b);
}

/** WCAG 2.1 contrast ratio, 1 to 21. */
export function contrastRatio(a, b) {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** WCAG 2.1: AA 4.5 (3 large), AAA 7 (4.5 large). */
export function wcagGrade(ratio, large) {
  if (ratio >= (large ? 4.5 : 7)) return 'AAA';
  if (ratio >= (large ? 3 : 4.5)) return 'AA';
  return 'fail';
}

// Machado, Oliveira & Fernandes (2009), "A Physiologically-based Model for Simulation of
// Color Vision Deficiency", IEEE TVCG 15(6). Severity 1.0 matrices, applied to linear RGB.
const CVD_MATRICES = {
  protanopia: [
    [0.152286, 1.052583, -0.204868],
    [0.114503, 0.786281, 0.099216],
    [-0.003882, -0.048116, 1.051998],
  ],
  deuteranopia: [
    [0.367322, 0.860646, -0.227968],
    [0.280085, 0.672501, 0.047413],
    [-0.01182, 0.04294, 0.968881],
  ],
  tritanopia: [
    [1.255528, -0.076749, -0.178779],
    [-0.078411, 0.930809, 0.147602],
    [0.004733, 0.691367, 0.3039],
  ],
};
export const CVD_TYPES = Object.keys(CVD_MATRICES);

/** Simulate how `c` looks under a color-vision deficiency; result is clamped sRGB. */
export function simulateCvd(c, type) {
  const m = CVD_MATRICES[type];
  if (!m) throw new Error(`unknown color-vision type: ${type}`);
  const lin = [toLinear(c.r), toLinear(c.g), toLinear(c.b)];
  const [r, g, b] = m.map((row) => fromLinear(row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]));
  return { r, g, b };
}

// sRGB -> CIE Lab, D65 white point.
function toLab(c) {
  const [r, g, b] = [toLinear(c.r), toLinear(c.g), toLinear(c.b)];
  const x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
  const y = 0.2126729 * r + 0.7151522 * g + 0.072175 * b;
  const z = (0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883;
  const f = (t) => (t > 216 / 24389 ? Math.cbrt(t) : (24389 / 27 * t + 16) / 116);
  const [fx, fy, fz] = [f(x), f(y), f(z)];
  return { L: 116 * fy - 16, a: 500 * (fx - fy), b: 200 * (fy - fz) };
}

const rad = (d) => (d * Math.PI) / 180;
const deg = (r) => (r * 180) / Math.PI;

/** CIEDE2000 between two Lab colors (Sharma, Wu & Dalal 2005). */
export function deltaE2000Lab(p, q) {
  const C1 = Math.hypot(p.a, p.b);
  const C2 = Math.hypot(q.a, q.b);
  const Cbar = (C1 + C2) / 2;
  const G = 0.5 * (1 - Math.sqrt(Cbar ** 7 / (Cbar ** 7 + 25 ** 7)));
  const a1 = (1 + G) * p.a;
  const a2 = (1 + G) * q.a;
  const Cp1 = Math.hypot(a1, p.b);
  const Cp2 = Math.hypot(a2, q.b);
  const hue = (b, a) => (b === 0 && a === 0 ? 0 : (deg(Math.atan2(b, a)) + 360) % 360);
  const h1 = hue(p.b, a1);
  const h2 = hue(q.b, a2);
  const dL = q.L - p.L;
  const dC = Cp2 - Cp1;
  let dh = 0;
  if (Cp1 * Cp2 !== 0) {
    dh = h2 - h1;
    if (dh > 180) dh -= 360;
    else if (dh < -180) dh += 360;
  }
  const dH = 2 * Math.sqrt(Cp1 * Cp2) * Math.sin(rad(dh / 2));
  const Lbar = (p.L + q.L) / 2;
  const Cpbar = (Cp1 + Cp2) / 2;
  let hbar = h1 + h2;
  if (Cp1 * Cp2 === 0) hbar = h1 + h2;
  else if (Math.abs(h1 - h2) <= 180) hbar = (h1 + h2) / 2;
  else hbar = (h1 + h2 + (h1 + h2 < 360 ? 360 : -360)) / 2;
  const T = 1 - 0.17 * Math.cos(rad(hbar - 30)) + 0.24 * Math.cos(rad(2 * hbar))
    + 0.32 * Math.cos(rad(3 * hbar + 6)) - 0.2 * Math.cos(rad(4 * hbar - 63));
  const dTheta = 30 * Math.exp(-(((hbar - 275) / 25) ** 2));
  const Rc = 2 * Math.sqrt(Cpbar ** 7 / (Cpbar ** 7 + 25 ** 7));
  const Sl = 1 + (0.015 * (Lbar - 50) ** 2) / Math.sqrt(20 + (Lbar - 50) ** 2);
  const Sc = 1 + 0.045 * Cpbar;
  const Sh = 1 + 0.015 * Cpbar * T;
  const Rt = -Math.sin(rad(2 * dTheta)) * Rc;
  return Math.sqrt(
    (dL / Sl) ** 2 + (dC / Sc) ** 2 + (dH / Sh) ** 2 + Rt * (dC / Sc) * (dH / Sh),
  );
}

/** CIEDE2000 between two sRGB colors. */
export function deltaE2000(a, b) {
  return deltaE2000Lab(toLab(a), toLab(b));
}

// Two colors that differ by less than this under a simulated color-vision deficiency are
// flagged as hard to tell apart. CIEDE2000 of about 2.3 is one just-noticeable difference;
// 10 is a conservative "clearly distinguishable at a glance" bar for UI states such as
// error vs success, so near-misses show up without flagging every similar-looking pair.
export const CVD_MIN_DELTA_E = 10;

/**
 * Pairs that become hard to tell apart under a color-vision deficiency.
 * One entry per (pair, type) below CVD_MIN_DELTA_E, worst (smallest) first.
 */
export function cvdFindings(pairs) {
  const out = [];
  for (const { a, b, colorA, colorB } of pairs) {
    for (const type of CVD_TYPES) {
      const deltaE = deltaE2000(simulateCvd(colorA, type), simulateCvd(colorB, type));
      if (deltaE < CVD_MIN_DELTA_E) out.push({ a, b, type, deltaE });
    }
  }
  return out.sort((x, y) => x.deltaE - y.deltaE);
}
