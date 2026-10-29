#!/usr/bin/env node
// replay-lens-foci.mjs — which TOPICS a repo's lenses ask about, sliced out of
// the shipping workflow.
//
// WHY IT EXISTS. `replay-lens-routing.mjs` pinned which AGENT reviews a repo
// (fixed 2026-08-10, after FE PR #1516 came back with zero candidates under the
// BE reviewer). That fix left a second copy of the same bug in place: until this
// region, `FOCI` was one hardcoded, BE-shaped dict handed to every lens
// regardless of repo — "performance" asked about N+1 queries and BullMQ jobs,
// "security" about auth middleware and injection, "dataContract" about Prisma
// and updatedAt handling. None of that has an answer in a React SPA, so a lens
// told to "Restrict this pass to ONLY: <that sentence>" returns nothing — not
// because the diff is clean, but because the question does not fit the
// codebase. Measured on FE PR #1554 (metrics.jsonl): widening the file
// partition so every focus read every file surfaced 4 real nits under
// correctness/dataContract/errorHandling while security and performance stayed
// at zero candidates — the same zero they carried across 6/6 and 5/6 FE runs
// respectively, unrelated to the file partition.
//
// That failure is silent by construction, same argument as the routing region:
// a misworded lens does not crash, does not warn, and returns a well-formed
// report — the only symptom is a yield of nothing, indistinguishable from a
// genuinely clean PR.
//
// The properties pinned here:
//   · each known repo maps to its OWN focus set, and the two are not the same
//     object — a copy-paste that pointed both rows at one set would restore the
//     original bug while leaving the table looking correct.
//   · the BE set is pinned to its EXACT pre-existing wording. This release is
//     FE-only by construction: a BE regression here means the pin broke, not
//     that BE earned a wording change nobody asked for.
//   · the FE set names none of the BE-specific vocabulary (N+1 queries, BullMQ,
//     DB calls, Prisma, updatedAt, auth middleware) — the exact tell that
//     explains the zero-yield lenses.
//   · every focus sentence in both sets round-trips through FOCI_LABEL to a
//     short key, and no two sentences collide — this is the attribution
//     invariant `mergeCandidates` depends on to tag a finding with its lens.
//   · an unknown repo falls back to the BE set rather than to undefined, same
//     posture as lensAgentFor and for the same reason: BE is the only
//     annotated corpus.
//   · selectFoci still forces the right lens onto the right risk surface per
//     repo, and does not throw on FE (which carries no `migration` focus)
//     when a migration-shaped path appears in its diff — that can't happen in
//     practice, but the function must not assume it.
//
// Usage: node eval/replay-lens-foci.mjs

import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { clientProfileByReviewer, loadClientProfiles } from "./client-profiles.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nlens foci — LENS-FOCI region of tribunal.js`);

const { api, violations } = loadRegion(WORKFLOW, "LENS-FOCI", [
  "fociFor",
  "fociLabelFor",
  "selectFoci",
  "validateProfile",
  "riskPathRules",
], ["PROFILE-LOOKUP"]);
const { selectFoci, validateProfile, riskPathRules } = api;
const PROFILES = loadClientProfiles();
for (const p of Object.values(PROFILES)) validateProfile(p, p.repo);
const fociFor = (r) => api.fociFor(PROFILES[r]);
const FOCI_BY_REPO = Object.fromEntries(Object.entries(PROFILES).map(([r, p]) => [r, api.fociFor(p)]));
// Per-repo now: each run labels its own lens plan from its own profile. The
// union keeps the "every sentence resolves, no collisions" check meaningful.
const FOCI_LABEL = Object.assign({}, ...Object.values(PROFILES).map((p) => api.fociLabelFor(p)));
const refusal = (fn) => {
  try {
    fn();
    return "";
  } catch (err) {
    return err.message;
  }
};

check(
  "the region is pure (no run state, no dispatch)",
  violations.length === 0,
  violations.join("; "),
);

const BE = clientProfileByReviewer("backend").profile.repo;
const FE = clientProfileByReviewer("frontend").profile.repo;
// A sibling repo of the same owner, and the FE repo's bare name.
const SIBLING = `${BE.split("/")[0]}/documentation`;
const FE_NAME = FE.split("/").pop();

console.log(`\n--- the check is not vacuous ---`);
check("the region exported a callable", typeof fociFor === "function");
check(
  "the table has both frozen repos",
  Object.keys(FOCI_BY_REPO).includes(BE) && Object.keys(FOCI_BY_REPO).includes(FE),
  Object.keys(FOCI_BY_REPO).join(", "),
);

console.log(`\n--- each repo gets its own focus set, and BE's is unchanged ---`);
const BE_PINNED = {
  correctness:
    "correctness and logic (control flow, error paths, edge cases, off-by-one)",
  dataContract:
    "data contracts and Prisma (type coercions, updatedAt handling, FE snake_case shape, schema alignment)",
  security:
    "security and authorization (auth middleware, role checks, input validation, injection)",
  performance:
    "performance (N+1 queries, unnecessary DB calls, BullMQ job efficiency)",
  errorHandling:
    "error handling and observability (error types, logger usage, missing catch paths)",
  migration:
    "data integrity and migration safety (rollback, backfill, nullability, destructive DDL, data loss on migrate)",
};
for (const key of Object.keys(BE_PINNED)) {
  check(
    `BE.${key} is byte-identical to the pre-existing wording`,
    fociFor(BE)[key] === BE_PINNED[key],
    fociFor(BE)[key],
  );
}
check(
  "BE and FE do NOT share the same object",
  fociFor(BE) !== fociFor(FE),
);
for (const key of ["security", "performance", "dataContract"]) {
  check(
    `FE.${key} differs from BE.${key}`,
    fociFor(FE)[key] !== fociFor(BE)[key],
    fociFor(FE)[key],
  );
}

console.log(`\n--- FE carries none of the BE-specific vocabulary ---`);
const feText = Object.values(fociFor(FE)).join(" \n ");
for (const tell of [
  "N+1 queries",
  "BullMQ",
  "DB calls",
  "Prisma",
  "updatedAt",
  "auth middleware",
]) {
  check(`FE foci do not mention "${tell}"`, !feText.includes(tell), feText);
}
check(
  "FE has no migration focus (no Prisma surface in this repo)",
  !("migration" in fociFor(FE)),
  JSON.stringify(Object.keys(fociFor(FE))),
);

console.log(`\n--- every focus sentence resolves to a short key, no collisions ---`);
{
  const allSentences = [
    ...Object.entries(fociFor(BE)),
    ...Object.entries(fociFor(FE)),
  ];
  const seen = new Map();
  for (const [key, sentence] of allSentences) {
    check(
      `FOCI_LABEL["${sentence.slice(0, 40)}..."] resolves to "${key}"`,
      FOCI_LABEL[sentence] === key,
      FOCI_LABEL[sentence],
    );
    if (seen.has(sentence) && seen.get(sentence) !== key) {
      failures.push(`sentence collision: "${sentence}" maps to both ${seen.get(sentence)} and ${key}`);
      console.log(`  FAIL  sentence collision between ${seen.get(sentence)} and ${key}`);
    }
    seen.set(sentence, key);
  }
}

// INTENDED DIFFERENCE from v82: an unknown repo used to fall back to the BE
// focus set. Now there are no foci without that repo's own profile.
console.log(`\n--- no profile, no foci: nothing falls back to the BE set ---`);
for (const bogus of [SIBLING, "", FE_NAME]) {
  check(`${JSON.stringify(bogus)} has no profile`, !(bogus in PROFILES));
}
check(
  "a profile reusing one focus sentence under two keys is refused (FOCI_LABEL would collide)",
  /FOCI_LABEL could not attribute it/.test(
    refusal(() => validateProfile({ ...PROFILES[FE], foci: { ...PROFILES[FE].foci, performance: PROFILES[FE].foci.security } }, FE)),
  ),
);
check(
  "a profile missing a required focus is refused",
  /foci\.security must be a non-empty sentence/.test(
    refusal(() => {
      const { security, ...rest } = PROFILES[FE].foci;
      return validateProfile({ ...PROFILES[FE], foci: rest }, FE);
    }),
  ),
);

const rulesFor = (r) => riskPathRules(PROFILES[r]);

console.log(`\n--- selectFoci still forces the right lens onto the right risk surface ---`);
check(
  "FE + an auth path forces the security focus first",
  selectFoci(["src/Components/auth/login.tsx"], 5, fociFor(FE), rulesFor(FE))[0] === fociFor(FE).security,
);
check(
  "BE + a migration path forces the migration focus first",
  selectFoci(["prisma/migrations/20260101_x.sql"], 5, fociFor(BE), rulesFor(BE))[0] === fociFor(BE).migration,
);
check(
  "BE + a payment path forces the dataContract focus",
  selectFoci(["src/services/v2/payment.service.ts"], 5, fociFor(BE), rulesFor(BE)).includes(fociFor(BE).dataContract),
);
{
  let threw = false;
  let result = null;
  try {
    result = selectFoci(["prisma/migrations/x.sql"], 5, fociFor(FE), rulesFor(FE));
  } catch {
    threw = true;
  }
  check(
    "FE does not throw on a migration-shaped path (it has no migration focus to force)",
    !threw,
    threw ? "threw" : JSON.stringify(result),
  );
}
check(
  "selectFoci never returns fewer than the file-partition budget when unforced",
  selectFoci(["src/Components/foo/bar.tsx"], 5, fociFor(FE), rulesFor(FE)).length === 5,
);


if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall lens-foci expectations hold");
