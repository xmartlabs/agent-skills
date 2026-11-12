import { useEffect, useState } from "react";
import { flows } from "../shell/flows";
import { useRoute } from "../shell/route";
import { specimenNames } from "../shell/specimens";
import { AuditPage, Badge, type BadgeTone } from "./AuditPage";
import { axeRows, type AxeRow } from "./axe-rows.mjs";

const TIMEOUT_MS = 20000;
const targets = [
  ...specimenNames.map((name) => `/components/${encodeURIComponent(name)}`),
  ...flows.map(({ name, meta }) => `/preview/${name}${meta.startRoute}`),
];

// axe impacts, worst first: critical and serious are failures, moderate is worth a look.
const IMPACT_TONES: Record<string, BadgeTone> = { critical: "bad", serious: "bad", moderate: "warn" };

type Outcome = { rows: AxeRow[]; error?: string; unsettled?: boolean };

// Loads one target in a hidden same-origin frame at desktop width and waits for the frame's axe report.
function checkTarget(target: string): Promise<Outcome> {
  return new Promise((resolve) => {
    const frame = document.createElement("iframe");
    frame.style.cssText = "position:absolute;left:-10000px;width:1280px;height:800px;border:0";
    frame.setAttribute("aria-hidden", "true");
    frame.tabIndex = -1;
    const finish = (outcome: Outcome) => {
      window.clearTimeout(timer);
      window.removeEventListener("message", listen);
      frame.remove();
      resolve(outcome);
    };
    const listen = (event: MessageEvent) => {
      const data = event.data;
      if (event.source !== frame.contentWindow || data?.type !== "design-space:axe" || data.target !== target.split("?")[0]) return;
      finish({ rows: axeRows(target, data.violations ?? []), error: data.error, unsettled: data.unsettled });
    };
    const timer = window.setTimeout(() => finish({ rows: [], error: "the page did not report in time" }), TIMEOUT_MS);
    window.addEventListener("message", listen);
    frame.src = `#${target}${target.includes("?") ? "&" : "?"}audit=a11y`;
    document.body.append(frame);
  });
}

export function Accessibility() {
  // "?audit=render-only" renders the page without loading any target: the smoke check uses it, so
  // its run time does not grow with the number of specimens and flows.
  const renderOnly = useRoute().query.get("audit") === "render-only";
  const [done, setDone] = useState(0);
  const [outcomes, setOutcomes] = useState<Array<[string, Outcome]>>([]);

  useEffect(() => {
    if (renderOnly) return;
    let cancelled = false;
    (async () => {
      for (const target of targets) {
        const outcome = await checkTarget(target);
        if (cancelled) return;
        setOutcomes((all) => [...all, [target, outcome]]);
        setDone((n) => n + 1);
      }
    })();
    return () => { cancelled = true; };
  }, [renderOnly]);

  const failing = outcomes.filter(([, { rows, error, unsettled }]) => rows.length > 0 || error || unsettled);
  return (
    <AuditPage
      title="Accessibility"
      lede="axe-core rules button-name, link-name, label, select-name and image-alt, on every specimen and the start page of every flow, at desktop width."
    >
      {renderOnly ? (
        <div className="ds-card ds-empty ds-muted-text">
          <p>
            Render-only mode: {targets.length} pages to check, none checked.{" "}
            <a className="ds-link" href="#/audits/accessibility">Run the audit</a>.
          </p>
        </div>
      ) : done < targets.length ? (
        <p className="ds-card ds-empty ds-muted-text" role="status" aria-busy="true">Checked {done} of {targets.length}.</p>
      ) : failing.length === 0 ? (
        <div className="ds-card ds-empty ds-muted-text">
          <p><Badge tone="ok">clean</Badge> No problems found in {specimenNames.length} specimens and {flows.length} prototypes.</p>
        </div>
      ) : (
        <>
          <p className="ds-summary">{failing.length} of {targets.length} pages have problems.</p>
          {failing.map(([target, { rows, error, unsettled }]) => (
            <section key={target} className="ds-section">
              <h2 className="ds-section-title">
                <a className="ds-link" href={`#${target}`}><code className="ds-code">{target}</code></a>
              </h2>
              {error && <p className="ds-summary"><Badge tone="bad">not checked</Badge> Could not check this page: {error}</p>}
              {unsettled && <p className="ds-summary"><Badge tone="warn">unsettled</Badge> Checked before the page settled; results may be incomplete.</p>}
              {rows.length > 0 && (
                <div className="ds-card ds-table-card">
                  <table className="ds-table">
                    <thead><tr><th>Rule</th><th>Impact</th><th>Element</th></tr></thead>
                    <tbody>
                      {rows.map((row, index) => (
                        <tr key={index}>
                          <td className="ds-nowrap">{row.rule}</td>
                          <td><Badge tone={IMPACT_TONES[row.impact] ?? "neutral"}>{row.impact}</Badge></td>
                          <td><code className="ds-code">{row.html}</code></td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </section>
          ))}
        </>
      )}
    </AuditPage>
  );
}
