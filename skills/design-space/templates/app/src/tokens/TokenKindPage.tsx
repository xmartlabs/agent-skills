import { useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { NotFound } from "../shell/Pages";
import { TOKEN_PAGES } from "../shell/nav.mjs";
import { THEME_EVENT } from "../shell/theme.mjs";
import { groupColors, ROLE_LABELS } from "./color-role.mjs";
import { emittedValue, liveValue, tokenMap, tokensByVar, type Token } from "./map";
import { summaryLine } from "./map-core.mjs";
import { SwatchTile, swatchesOf } from "./Swatch";

const SAMPLE = "The quick brown fox";

const nameOf = (token: Token) => token.cssVar ?? token.name;
const keyOf = (token: Token) => `${token.source}:${nameOf(token)}`;

// The value to paint with: the variable when the page emits it (it follows the theme), else the
// declared value with var() chains resolved.
function paintValue(token: Token, byVar: Map<string, Token>) {
  return token.cssVar && emittedValue(token) ? `var(${token.cssVar})` : liveValue(token, byVar);
}

// The value to print: computed for the active theme, else resolved from the source.
const shownValue = (token: Token, byVar: Map<string, Token>) => emittedValue(token) || liveValue(token, byVar);

function Notes({ token }: { token: Token }) {
  const fromSource = Boolean(token.cssVar) && !emittedValue(token);
  return (
    <>
      {fromSource && <span className="ds-token-note">value from source</span>}
      {token.overrides && <span className="ds-token-note">redefines a product token</span>}
    </>
  );
}

function Section({ title, count, children }: { title: string; count: number; children: ReactNode }) {
  return (
    <section className="ds-section ds-token-section">
      <h2 className="ds-section-title">
        {title} <span className="ds-count">{count}</span>
      </h2>
      {children}
    </section>
  );
}

function Colors({ tokens, byVar }: { tokens: Token[]; byVar: Map<string, Token> }) {
  // Read every color first: a tile names the palette token with its hex in the active theme.
  const swatches = new Map(swatchesOf(tokens, byVar).map((swatch) => [swatch.token, swatch]));
  return (
    <>
      {groupColors(tokens).map(([role, list]) => (
        <Section key={role} title={ROLE_LABELS[role]} count={list.length}>
          <ul className="ds-swatch-grid">
            {list.map((token) => <SwatchTile key={keyOf(token)} swatch={swatches.get(token)!} />)}
          </ul>
        </Section>
      ))}
    </>
  );
}

// A font family value without its fallbacks: `"Inter", ui-sans-serif` -> `Inter`.
const familyName = (value: string) => value.split(",")[0].trim().replace(/^["']|["']$/g, "");

// Tailwind v4 pairs a size with modifiers (`--text-lg--line-height`); those are not sizes.
const isModifier = (token: Token) => /--/.test(token.name);

function Typography({ tokens, byVar }: { tokens: Token[]; byVar: Map<string, Token> }) {
  const fonts = tokens.filter((token) => token.kind === "font");
  const sizes = tokens.filter((token) => token.kind === "text");
  return (
    <>
      {fonts.length > 0 && (
        <Section title="Font families" count={fonts.length}>
          <ul className="ds-font-grid">
            {fonts.map((token) => (
              <li key={keyOf(token)} className="ds-card ds-font-card" data-token-kind={token.kind}>
                <span className="ds-font-head">
                  <span className="ds-font-family">{familyName(shownValue(token, byVar))}</span>
                  <code className="ds-token-name" data-token-name>{nameOf(token)}</code>
                </span>
                <span className="ds-font-sample" style={{ fontFamily: paintValue(token, byVar) }}>Ag {SAMPLE}</span>
                <span className="ds-token-value">{shownValue(token, byVar)}</span>
                <Notes token={token} />
              </li>
            ))}
          </ul>
        </Section>
      )}
      {sizes.length > 0 && (
        <Section title="Text sizes" count={sizes.length}>
          <ul className="ds-card ds-scale-list">
            {sizes.map((token) => (
              <li key={keyOf(token)} className="ds-scale-row" data-token-kind={token.kind}>
                <code className="ds-token-name" data-token-name>{nameOf(token)}</code>
                <span className="ds-scale-sample" style={isModifier(token) ? undefined : { fontSize: paintValue(token, byVar) }}>
                  {isModifier(token) ? "Modifier of a text size" : SAMPLE}
                </span>
                <span className="ds-token-value">
                  {shownValue(token, byVar)}
                  <Notes token={token} />
                </span>
              </li>
            ))}
          </ul>
        </Section>
      )}
    </>
  );
}

// Spacing bars, radius boxes and shadow boxes: one tile per token.
function Visual({ token, byVar }: { token: Token; byVar: Map<string, Token> }) {
  const v = paintValue(token, byVar);
  const styles: Partial<Record<Token["kind"], CSSProperties>> = {
    spacing: { width: v },
    radius: { borderRadius: v },
    shadow: { boxShadow: v },
  };
  return <span className={`ds-visual ds-visual-${token.kind}`} style={styles[token.kind]} />;
}

function Boxes({ title, tokens, byVar }: { title: string; tokens: Token[]; byVar: Map<string, Token> }) {
  return (
    <Section title={title} count={tokens.length}>
      <ul className="ds-box-grid">
        {tokens.map((token) => (
          <li key={keyOf(token)} className="ds-card ds-box-tile" data-token-kind={token.kind}>
            <span className="ds-box-stage"><Visual token={token} byVar={byVar} /></span>
            <code className="ds-token-name" data-token-name>{nameOf(token)}</code>
            <span className="ds-token-value">{shownValue(token, byVar)}</span>
            <Notes token={token} />
          </li>
        ))}
      </ul>
    </Section>
  );
}

function Bars({ tokens, byVar }: { tokens: Token[]; byVar: Map<string, Token> }) {
  return (
    <Section title="Spacing scale" count={tokens.length}>
      <ul className="ds-card ds-scale-list">
        {tokens.map((token) => (
          <li key={keyOf(token)} className="ds-scale-row" data-token-kind={token.kind}>
            <code className="ds-token-name" data-token-name>{nameOf(token)}</code>
            <span className="ds-bar-track"><Visual token={token} byVar={byVar} /></span>
            <span className="ds-token-value">
              {shownValue(token, byVar)}
              <Notes token={token} />
            </span>
          </li>
        ))}
      </ul>
    </Section>
  );
}

function Table({ tokens, byVar }: { tokens: Token[]; byVar: Map<string, Token> }) {
  return (
    <Section title="Tokens" count={tokens.length}>
      <div className="ds-card ds-table-card">
        <table className="ds-table">
          <thead>
            <tr><th>Name</th><th>Kind</th><th>Value</th><th>Source</th></tr>
          </thead>
          <tbody>
            {tokens.map((token) => (
              <tr key={keyOf(token)} data-token-kind={token.kind}>
                <td><code className="ds-token-name" data-token-name>{nameOf(token)}</code></td>
                <td>{token.kind}</td>
                <td><code className="ds-code">{shownValue(token, byVar)}</code> <Notes token={token} /></td>
                <td>{token.source}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Section>
  );
}

/** `#/tokens/<kind>`: the tokens of one page of TOKEN_PAGES, drawn for what they are. */
export function TokenKindPage({ kind }: { kind: string }) {
  const page = TOKEN_PAGES.find(({ slug }) => slug === kind);
  const tokens = useMemo(() => (page ? tokenMap().filter((token) => page.kinds.includes(token.kind)) : []), [page]);
  const byVar = useMemo(() => tokensByVar(), []);
  const [, refresh] = useState(0);
  // Computed values and hex codes change with the theme.
  useEffect(() => {
    const on = () => refresh((n) => n + 1);
    window.addEventListener(THEME_EVENT, on);
    return () => window.removeEventListener(THEME_EVENT, on);
  }, []);

  if (!page) return <NotFound what="token group" />;
  const props = { tokens, byVar };
  return (
    <div className="ds-page">
      <header className="ds-page-head">
        <h1 className="ds-page-title">{page.label}</h1>
        {tokens.length > 0 && <p className="ds-lede">{summaryLine(tokens)}</p>}
      </header>
      {tokens.length === 0 ? (
        <p className="ds-card ds-empty ds-muted-text">No tokens of this kind</p>
      ) : page.slug === "color" ? (
        <Colors {...props} />
      ) : page.slug === "typography" ? (
        <Typography {...props} />
      ) : page.slug === "spacing" ? (
        <Bars {...props} />
      ) : page.slug === "radius" ? (
        <Boxes title="Radius" {...props} />
      ) : page.slug === "shadow" ? (
        <Boxes title="Shadow" {...props} />
      ) : (
        <Table {...props} />
      )}
    </div>
  );
}
