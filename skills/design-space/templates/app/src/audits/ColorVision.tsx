import { CVD_MIN_DELTA_E, CVD_TYPES, cvdFindings, simulateCvd } from "./color.mjs";
import { AuditPage, Badge, OverrideNotice, Unreadable, swatch, useActiveTheme } from "./AuditPage";
import { colorVisionPairs, type Pair } from "./pairs.mjs";
import { tokenColorReader } from "./token-color";
import { tokenMap } from "../tokens/map";
import pairsJson from "../../audits/contrast-pairs.json";

export function ColorVision() {
  useActiveTheme(); // re-read the colors when the theme changes
  const readable: Array<{ a: string; b: string; colorA: ReturnType<typeof simulateCvd>; colorB: ReturnType<typeof simulateCvd> }> = [];
  const unreadable: string[] = [];
  const tokens = tokenMap();
  // A CSS token resolves through its variable (or its declared value when not emitted); a
  // theme-object token has none, so by its value.
  const resolve = tokenColorReader(tokens);
  for (const { a, b } of colorVisionPairs(tokens, pairsJson as Pair[])) {
    const colorA = resolve(a);
    const colorB = resolve(b);
    if (colorA && colorB) readable.push({ a, b, colorA, colorB });
    else unreadable.push(`${a} / ${b}`);
  }
  const findings = cvdFindings(readable);
  const flagged = readable.filter(({ a, b }) => findings.some((f) => f.a === a && f.b === b));
  return (
    <AuditPage
      title="Color vision"
      lede={
        <>
          Pairs that look too alike under protanopia, deuteranopia or tritanopia (Machado 2009). A pair is flagged when
          the CIEDE2000 distance drops below {CVD_MIN_DELTA_E}.
        </>
      }
    >
      <OverrideNotice tokens={tokens} />
      <p className="ds-summary">{readable.length} pairs checked, {flagged.length} flagged.</p>
      {readable.length > 0 && flagged.length === 0 && (
        <div className="ds-card ds-empty ds-muted-text">
          <p><Badge tone="ok">none flagged</Badge> Every pair stays distinguishable under the three simulations.</p>
        </div>
      )}
      {flagged.map(({ a, b, colorA, colorB }) => (
        <section key={`${a}|${b}`} className="ds-section">
          <h2 className="ds-section-title"><code className="ds-code">{a}</code> and <code className="ds-code">{b}</code></h2>
          <div className="ds-card ds-table-card">
            <table className="ds-table">
              <thead><tr><th>Simulation</th><th>Colors as seen</th><th>Result</th></tr></thead>
              <tbody>
                {CVD_TYPES.map((type) => {
                  const finding = findings.find((f) => f.a === a && f.b === b && f.type === type);
                  return (
                    <tr key={type}>
                      <td>{type}</td>
                      <td>
                        <span className="ds-cvd-swatch" title={a} style={{ background: swatch(simulateCvd(colorA, type)) }} />
                        <span className="ds-cvd-swatch" title={b} style={{ background: swatch(simulateCvd(colorB, type)) }} />
                      </td>
                      <td>
                        {finding
                          ? <Badge tone="warn">flagged, distance {finding.deltaE.toFixed(1)}</Badge>
                          : <Badge tone="ok">distinguishable</Badge>}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>
      ))}
      <Unreadable names={unreadable} />
    </AuditPage>
  );
}
