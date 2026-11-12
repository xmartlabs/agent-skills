import { contrastRatio, wcagGrade } from "./color.mjs";
import { AuditPage, Badge, OverrideNotice, Unreadable, swatch, useActiveTheme } from "./AuditPage";
import type { Pair } from "./pairs.mjs";
import { tokenColorReader } from "./token-color";
import { hexOf } from "../tokens/color-role.mjs";
import { tokenMap } from "../tokens/map";
import pairsJson from "../../audits/contrast-pairs.json";

const pairs = pairsJson as Pair[];
type RGB = { r: number; g: number; b: number };

// A pass is green, a fail red; the text stays lower case for the tests that read it.
const Result = ({ pass }: { pass: boolean }) => <Badge tone={pass ? "ok" : "bad"}>{pass ? "pass" : "fail"}</Badge>;

export function Contrast() {
  useActiveTheme(); // re-read the colors when the theme changes
  const rows: Array<{ pair: Pair; ratio: number; grade: string; fg: RGB; bg: RGB }> = [];
  const unreadable: string[] = [];
  const tokens = tokenMap();
  const readColor = tokenColorReader(tokens);
  for (const pair of pairs) {
    const fg = readColor(pair.foreground);
    const bg = readColor(pair.background);
    if (!fg || !bg) {
      unreadable.push(pair.label);
      continue;
    }
    const ratio = contrastRatio(fg, bg);
    rows.push({ pair, ratio, grade: wcagGrade(ratio, pair.large), fg, bg });
  }
  rows.sort((x, y) => Number(y.grade === "fail") - Number(x.grade === "fail") || x.ratio - y.ratio);
  const failing = rows.filter(({ grade }) => grade === "fail").length;
  return (
    <AuditPage
      title="Contrast"
      lede="WCAG 2.1 contrast of text and surface colors (AA 4.5:1, 3:1 large text; AAA 7:1, 4.5:1 large text)."
    >
      <OverrideNotice tokens={tokens} />
      {pairs.length === 0 && (
        <div className="ds-card ds-empty ds-muted-text">
          <p>No pairs to check. Add pairs to <code className="ds-code">audits/contrast-pairs.json</code>.</p>
        </div>
      )}
      {rows.length > 0 && (
        <>
          <p className="ds-summary">{rows.length} pairs checked, {failing} below AA.</p>
          <div className="ds-card ds-table-card">
            <table className="ds-table">
              <thead><tr><th>Pair</th><th>Sample</th><th>Ratio</th><th>AA</th><th>AAA</th></tr></thead>
              <tbody>
                {rows.map(({ pair, ratio, grade, fg, bg }, index) => (
                  <tr key={`${index}:${pair.foreground}|${pair.background}`}>
                    <td>
                      <span className="ds-cell-main" data-pair-label>{pair.label}{pair.large ? " (large text)" : ""}</span>
                      <span className="ds-cell-sub">{hexOf(fg)} on {hexOf(bg)}</span>
                    </td>
                    <td><span className="ds-sample" style={{ color: swatch(fg), background: swatch(bg) }}>Aa</span></td>
                    <td className="ds-num">{ratio.toFixed(2)}:1</td>
                    <td><Result pass={grade !== "fail"} /></td>
                    <td><Result pass={grade === "AAA"} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
      <Unreadable names={unreadable} />
    </AuditPage>
  );
}
