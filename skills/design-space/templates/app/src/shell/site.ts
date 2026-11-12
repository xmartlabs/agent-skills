import { meta } from "virtual:design-space/catalog";
import { AUDITS } from "../audits/AuditsIndex";
import { tokenMap } from "../tokens/map";
import { flows } from "./flows";
import { buildNav, productName, tokenPages } from "./nav.mjs";
import { sharedNames } from "./shared";
import { specimenNames } from "./specimens";

// Everything the sidebar and home page index, computed once: pages appear by existing.
const kinds = [...new Set(tokenMap().map((token) => token.kind))];

export const site = {
  product: productName(meta),
  branch: meta.branch,
  tokenCount: tokenMap().length,
  tokenPages: tokenPages(kinds),
  nav: buildNav({
    tokenKinds: kinds,
    components: specimenNames,
    shared: sharedNames,
    flows: flows.map(({ name, meta: flow }) => ({ name, title: flow.title })),
    audits: AUDITS,
  }),
};
