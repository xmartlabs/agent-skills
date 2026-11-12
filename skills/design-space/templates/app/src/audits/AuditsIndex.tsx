import { ArrowRightIcon } from "../shell/icons";
import { Badge } from "./AuditPage";

export const AUDITS = [
  { name: "accessibility", title: "Accessibility", summary: "axe-core rules on every specimen and flow page." },
  { name: "color-vision", title: "Color vision", summary: "Status and chart colors that look alike under color-vision deficiency." },
  { name: "contrast", title: "Contrast", summary: "WCAG contrast of text on its surface for the active theme." },
  { name: "tokens", title: "Token usage", summary: "Tokens with no value and tokens that share a value." },
  { name: "components", title: "Component coverage", summary: "Which product components a specimen covers." },
];

export function AuditsIndex() {
  return (
    <div className="ds-page">
      <header className="ds-page-head">
        <h1 className="ds-page-title">Audits</h1>
        <p className="ds-lede">Checks on the tokens, components and pages of the design space.</p>
        <p className="ds-head-meta">
          <Badge>Report only</Badge>
          <span>Does not block a pull request.</span>
        </p>
      </header>
      <ul className="ds-card-grid ds-link-grid">
        {AUDITS.map(({ name, title, summary }) => (
          <li key={name}>
            <a className="ds-card ds-link-card" href={`#/audits/${name}`}>
              <span className="ds-link-card-title">
                {title}
                <ArrowRightIcon size={16} />
              </span>
              <span className="ds-card-text">{summary}</span>
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}
