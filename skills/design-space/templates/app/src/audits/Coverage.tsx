import { components, componentsFolder } from "virtual:design-space/catalog";
import { labelOf } from "../shell/nav.mjs";
import { specimenName } from "../shell/specimen-name.mjs";
import { AuditPage, Badge, type BadgeTone } from "./AuditPage";

const TONES: Record<string, BadgeTone> = { covered: "ok", excluded: "neutral", missing: "bad" };

export function Coverage() {
  const count = (status: string) => components.filter((row) => row.status === status).length;
  return (
    <AuditPage title="Component coverage" lede={componentsFolder === null ? undefined : `Product components in ${componentsFolder} and the specimens that cover them.`}>
      {componentsFolder === null ? (
        <div className="ds-card ds-empty ds-muted-text">
          <p>No components folder is set in .design-space.json (paths.components), so there is nothing to count.</p>
        </div>
      ) : (
        <>
          <p className="ds-summary">Covered: {count("covered")}. Excluded: {count("excluded")}. Missing: {count("missing")}.</p>
          <div className="ds-card ds-table-card">
            <table className="ds-table">
              <thead><tr><th>Component</th><th>Status</th><th>Specimen or reason</th></tr></thead>
              <tbody>
                {components.map((row) => {
                  // row.by is the covering specimen file ("specimens/forms/Field.tsx"): link to its page.
                  const specimen = row.by ? specimenName(`/${row.by}`) : null;
                  return (
                    <tr key={row.file}>
                      <td><code className="ds-code">{row.file}</code></td>
                      <td><Badge tone={TONES[row.status] ?? "neutral"}>{row.status}</Badge></td>
                      <td>
                        {specimen ? (
                          <a className="ds-link" href={`#/components/${encodeURIComponent(specimen)}`}>{labelOf(specimen)}</a>
                        ) : row.status === "excluded" ? (
                          <span className="ds-muted-text">{row.reason ?? "no reason given"}</span>
                        ) : (
                          <span className="ds-muted-text">No specimen yet</span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </>
      )}
    </AuditPage>
  );
}
