import type { ReactNode } from "react";
import { CanvasBoundary } from "./CanvasBoundary";
import { labelOf } from "../shell/nav.mjs";
import { NotFound } from "../shell/Pages";
import { sharedComponents } from "../shell/shared";
import { Specimen } from "../shell/Specimen";
import { specimens, type SpecimenModule } from "../shell/specimens";
import { Playground } from "./Playground";
import { tagNameOf } from "./snippet.mjs";

// One page per specimen (#/components/<name>) and per library/ component (#/shared/<name>): title
// and description, the specimens in a card, and a Playground when the file exports one. The
// specimens and the live preview render in .ds-canvas, which the shell does not style. A component
// that throws is caught at the canvas, so the shell stays.
const PROPLESS_HINT = "Export a default component that renders without props to preview it.";

function ComponentLayout({ name, module, hint, children }: { name: string; module: SpecimenModule; hint?: string; children?: ReactNode }) {
  const title = module.meta?.title?.trim() || labelOf(name);
  const description = module.meta?.description?.trim();
  const playground = module.playground;
  return (
    <div className="ds-page">
      <header className="ds-page-head">
        <h1 className="ds-page-title">{title}</h1>
        {description && <p className="ds-lede">{description}</p>}
      </header>
      <section className="ds-section" aria-labelledby="ds-specimens">
        <h2 className="ds-section-title" id="ds-specimens">Specimens</h2>
        {children === undefined ? (
          <section role="note" className="ds-card ds-note">
            <p className="ds-note-text">{hint}</p>
          </section>
        ) : (
          <CanvasBoundary key={name} name={name} hint={hint}>
            <div className="ds-card ds-canvas-card">
              <div className="ds-canvas">{children}</div>
            </div>
          </CanvasBoundary>
        )}
      </section>
      {playground?.component && (
        <section className="ds-section" aria-labelledby="ds-playground">
          <h2 className="ds-section-title" id="ds-playground">Playground</h2>
          <p className="ds-section-lede">Change the props and see the component update live.</p>
          <Playground
            key={name}
            spec={playground}
            tag={tagNameOf(playground.component.displayName, name)}
          />
        </section>
      )}
    </div>
  );
}

// An audit frame (embedded) loads only the specimen, so axe judges the component, not the shell.
export function ComponentPage({ name, embedded }: { name: string; embedded: boolean }) {
  if (embedded) {
    return (
      <CanvasBoundary key={name} name={name}>
        <div className="ds-canvas"><Specimen name={name} /></div>
      </CanvasBoundary>
    );
  }
  if (!Object.hasOwn(specimens, name)) return <NotFound what="component" />;
  return <ComponentLayout name={name} module={specimens[name]}><Specimen name={name} /></ComponentLayout>;
}

// Every library/ file the sidebar lists has a page. One with no default export shows how to get a
// preview; a default that needs props throws, and the boundary shows the same hint.
export function SharedPage({ name }: { name: string }) {
  const module = Object.hasOwn(sharedComponents, name) ? sharedComponents[name] : undefined;
  if (!module) return <NotFound what="shared component" />;
  const Component = module.default;
  return (
    <ComponentLayout name={name} module={module} hint={PROPLESS_HINT}>
      {Component ? <Component /> : undefined}
    </ComponentLayout>
  );
}
