import { readColor, readColorValue } from "../audits/color.mjs";
import { hexOf, paletteRefOf, themeRefOf } from "./color-role.mjs";
import { emittedValue, liveValue, type Token } from "./map";
import { cssColorOf } from "./parse.mjs";

type RGB = { r: number; g: number; b: number };
type Swatch = { token: Token; paint: string; hex: string | null; fromSource: boolean };

const hexOrNull = (rgb: RGB | null) => (rgb ? hexOf(rgb) : null);

/**
 * What a color tile shows for the active theme. A variable the page emits is read live; one it
 * does not emit (Tailwind v4 `@theme`, `@theme inline`) or a theme-object token is read from its
 * declared value, with var() chains resolved, and marked as coming from the source.
 */
function swatchOf(token: Token, byVar: Map<string, Token>): Swatch {
  const emitted = emittedValue(token);
  const fromSource = Boolean(token.cssVar) && !emitted;
  const value = emitted || liveValue(token, byVar);
  const live = !fromSource && token.cssVar ? (readColor(token.cssVar) as RGB | null) : null;
  return { token, paint: cssColorOf(value), hex: hexOrNull(live ?? (readColorValue(cssColorOf(value)) as RGB | null)), fromSource };
}

/** One swatch per token, each with the token it points to in the active theme (see themeRefOf). */
export function swatchesOf(tokens: Token[], byVar: Map<string, Token>) {
  const swatches = tokens.map((token) => swatchOf(token, byVar));
  const hexByName = new Map<string, string>();
  for (const { token, hex } of swatches) if (hex && !hexByName.has(token.name)) hexByName.set(token.name, hex);
  return swatches.map((swatch) => {
    const ref = paletteRefOf(swatch.token.value);
    const declaredHex = ref ? null : hexOrNull(readColorValue(cssColorOf(swatch.token.value)) as RGB | null);
    return { ...swatch, ref: themeRefOf({ ref, declaredHex, hex: swatch.hex }, hexByName) };
  });
}

export function SwatchTile({ swatch }: { swatch: Swatch & { ref: string | null } }) {
  const { token, paint, hex, fromSource, ref } = swatch;
  const name = token.cssVar ?? token.name;
  return (
    <li className="ds-swatch" data-token-kind={token.kind} title={name}>
      <span className="ds-swatch-color" style={{ background: paint }} />
      <span className="ds-swatch-body">
        <code className="ds-swatch-name" data-token-name>{name}</code>
        <span className="ds-swatch-meta">{ref ?? "Literal value"}</span>
        <span className="ds-swatch-hex">{hex ?? "Not a color"}</span>
        {fromSource && <span className="ds-swatch-note">value from source</span>}
        {token.overrides && <span className="ds-swatch-note">redefines a product token</span>}
      </span>
    </li>
  );
}
