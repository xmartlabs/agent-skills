import type { ComponentType, ReactNode } from "react";
import { AUDITS } from "../audits/AuditsIndex";
import { flows } from "./flows";
import { ArrowRightIcon, CheckIcon, ComponentIcon, PaletteIcon, PlayIcon } from "./icons";
import { labelOf } from "./nav.mjs";
import { site } from "./site";
import { specimenNames } from "./specimens";

type Link = { href: string; label: string };
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
const MAX_LINKS = 4;

function Card({ icon: Icon, title, summary, links, empty }: { icon: ComponentType; title: string; summary: ReactNode; links: Link[]; empty?: ReactNode }) {
  return (
    <section className="ds-card ds-home-card" aria-labelledby={`ds-card-${title}`}>
      <span className="ds-icon-tile"><Icon /></span>
      <h2 className="ds-card-title" id={`ds-card-${title}`}>{title}</h2>
      <p className="ds-card-text">{summary}</p>
      {links.length > 0 ? (
        <ul className="ds-link-list">
          {links.map(({ href, label }) => (
            <li key={href}>
              <a className="ds-link-row" href={href}>
                <span>{label}</span>
                <ArrowRightIcon size={14} />
              </a>
            </li>
          ))}
        </ul>
      ) : (
        empty && <p className="ds-card-empty">{empty}</p>
      )}
    </section>
  );
}

export function Home() {
  const kinds = site.tokenPages;
  return (
    <div className="ds-home">
      <header className="ds-home-head">
        <span className="ds-brand-mark ds-brand-mark-lg" aria-hidden="true">{site.product.charAt(0).toUpperCase()}</span>
        <h1 className="ds-home-title">{site.product}</h1>
        <p className="ds-lede">The design system, live components, prototypes and audits, in one place.</p>
      </header>
      <div className="ds-card-grid">
        <Card
          icon={PaletteIcon}
          title="Tokens"
          summary={kinds.length ? `${plural(site.tokenCount, "token", "tokens")} in ${plural(kinds.length, "group", "groups")}.` : "No tokens yet."}
          links={kinds.slice(0, MAX_LINKS).map(({ slug, label }) => ({ href: `#/tokens/${slug}`, label }))}
          empty={<>Connect a product token file in <code className="ds-code">src/product-tokens.ts</code>.</>}
        />
        <Card
          icon={ComponentIcon}
          title="Components"
          summary={specimenNames.length ? `${plural(specimenNames.length, "component", "components")} with live specimens.` : "No specimens yet."}
          links={specimenNames.slice(0, MAX_LINKS).map((name) => ({ href: `#/components/${encodeURIComponent(name)}`, label: labelOf(name) }))}
          empty={<>Add one to <code className="ds-code">specimens/</code>.</>}
        />
        <Card
          icon={PlayIcon}
          title="Prototypes"
          summary={flows.length ? `${plural(flows.length, "prototype", "prototypes")} with mocked data.` : "No prototypes yet."}
          links={[{ href: "#/prototypes", label: "All prototypes" }]}
        />
        <Card
          icon={CheckIcon}
          title="Audits"
          summary={`${plural(AUDITS.length, "report-only check", "report-only checks")} on tokens, components and pages.`}
          links={[{ href: "#/audits", label: "All audits" }]}
        />
      </div>
    </div>
  );
}
