import type { ComponentType } from "react";
import { Accessibility } from "../audits/Accessibility";
import { AuditsIndex } from "../audits/AuditsIndex";
import { ColorVision } from "../audits/ColorVision";
import { Contrast } from "../audits/Contrast";
import { Coverage } from "../audits/Coverage";
import { TokenUsage } from "../audits/TokenUsage";
import { auditName } from "./route";

const PAGES: Record<string, ComponentType> = {
  accessibility: Accessibility,
  "color-vision": ColorVision,
  contrast: Contrast,
  tokens: TokenUsage,
  components: Coverage,
};

export function Audits({ path }: { path: string }) {
  const name = auditName(path);
  if (name === null) return <AuditsIndex />;
  const Page = Object.hasOwn(PAGES, name) ? PAGES[name] : null;
  if (Page) return <Page />;
  return (
    <div className="ds-page">
      <header className="ds-page-head">
        <a className="ds-link ds-back" href="#/audits">All audits</a>
        <h1 className="ds-page-title">Audit not found</h1>
        <p className="ds-lede">There is no audit named {name}.</p>
      </header>
    </div>
  );
}
