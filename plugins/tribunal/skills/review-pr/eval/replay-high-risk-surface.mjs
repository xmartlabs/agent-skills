#!/usr/bin/env node
// replay-high-risk-surface.mjs — `touchesHighRiskSurface` was defined once, in
// backend vocabulary, for both repos.
//
// The defect (criterion SO-18, confirmed twice on 2026-09-07): one rigid
// definition forced onto structurally different inputs. The shared lens prompt
// said `true if it involves prisma/schema, migrations, auth, payments, mass
// delete/update` — and prisma, migrations and payments do not exist in the
// frontend repo. An FE lens was handed a field it could almost never set, and
// the field is not decorative: it gates escalation to a second vote
// (shouldEscalate), it chooses Codex's reasoning effort, and it decides whether
// Gemini is dispatched at all. So frontend findings were structurally excluded
// from the harness's own deeper treatment, and the exclusion was invisible —
// indistinguishable from the frontend simply never touching anything risky.
//
// The focus sentences were already keyed by repo. This is the same treatment
// for the one sentence that was not, and the checks below are mostly about the
// property that matters: the two repos must not describe the same surfaces.
//
// Usage: node eval/replay-high-risk-surface.mjs

import { readFileSync } from "node:fs";
import { loadRegion } from "./extract-pure-region.mjs";
import { clientProfileByReviewer, loadClientProfiles } from "./client-profiles.mjs";
import { WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

let loaded;
try {
  loaded = loadRegion(WORKFLOW, "HIGH-RISK-SURFACE", [
    "highRiskSurfaceFor",
    "validateProfile",
    "riskPathTouched",
    "riskPathRules",
  ], ["PROFILE-LOOKUP"]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const PROFILES = loadClientProfiles();
for (const p of Object.values(PROFILES)) api.validateProfile(p, p.repo);
const highRiskSurfaceFor = (r) => api.highRiskSurfaceFor(PROFILES[r]);
const HIGH_RISK_SURFACE_BY_REPO = Object.fromEntries(
  Object.entries(PROFILES).map(([r, p]) => [r, api.highRiskSurfaceFor(p)]),
);
const src = readFileSync(WORKFLOW, "utf8");

const BE = clientProfileByReviewer("backend").profile.repo;
const FE = clientProfileByReviewer("frontend").profile.repo;

console.log(`\nHIGH-RISK-SURFACE region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

console.log(`\n--- both repos are covered, and differently ---`);
{
  check("the backend has a definition", !!HIGH_RISK_SURFACE_BY_REPO[BE]);
  check("the frontend has one too", !!HIGH_RISK_SURFACE_BY_REPO[FE]);
  check(
    "they are not the same sentence — the whole point of the fix",
    HIGH_RISK_SURFACE_BY_REPO[BE] !== HIGH_RISK_SURFACE_BY_REPO[FE],
  );
  check(
    "the backend keeps the wording it always had",
    HIGH_RISK_SURFACE_BY_REPO[BE] ===
      "prisma/schema, migrations, auth, payments, mass delete/update",
  );
  check(
    "the frontend definition names no backend-only surface",
    !/prisma|migration/i.test(HIGH_RISK_SURFACE_BY_REPO[FE]),
    HIGH_RISK_SURFACE_BY_REPO[FE],
  );
  check(
    "…and does name surfaces that actually exist in a React app",
    /unsanitised HTML|browser storage|open redirect/i.test(
      HIGH_RISK_SURFACE_BY_REPO[FE],
    ),
  );
  check(
    "both still cover auth, which is high-risk in either repo",
    /auth/i.test(HIGH_RISK_SURFACE_BY_REPO[BE]) &&
      /auth/i.test(HIGH_RISK_SURFACE_BY_REPO[FE]),
  );
  check(
    "every definition is a non-empty sentence, not a placeholder",
    Object.values(HIGH_RISK_SURFACE_BY_REPO).every(
      (v) => typeof v === "string" && v.trim().length > 20,
    ),
  );
}

// INTENDED DIFFERENCE from v82: unknown repos used to fall back to the backend
// definition. Now a repo without a profile has no definition at all.
console.log(`\n--- no profile, no definition: nothing falls back to the backend sentence ---`);
{
  check("the backend resolves to its own", highRiskSurfaceFor(BE) === HIGH_RISK_SURFACE_BY_REPO[BE]);
  check("the frontend resolves to its own", highRiskSurfaceFor(FE) === HIGH_RISK_SURFACE_BY_REPO[FE]);
  for (const [label, value] of [
    ["an unknown repo", "someorg/some-service"],
    ["undefined", undefined],
    ["null", null],
    ["the empty string", ""],
  ]) {
    check(`${label} has no profile to borrow from`, !(value in PROFILES));
  }
  let refused = "";
  try {
    api.validateProfile({ ...PROFILES[FE], highRiskSurface: "" }, FE);
  } catch (err) {
    refused = err.message;
  }
  check(
    "a profile with an empty definition is refused, so undefined never reaches the prompt",
    /highRiskSurface must be a non-empty sentence/.test(refused),
    refused,
  );
}

console.log(`\n--- the prompt actually asks for it, and the field still gates things ---`);
{
  check(
    "the lens prompt interpolates the per-repo definition instead of a fixed sentence",
    /touchesHighRiskSurface: true if it involves \$\{p\.highRiskSurface\}/.test(src) &&
      /highRiskSurface: highRiskSurfaceFor\(PROFILE\),/.test(src),
  );
  const rules = api.riskPathRules({ riskPaths: [{ focus: "security", pattern: "auth|role" }, { focus: "dataContract", pattern: "invoice|payment" }] });
  check("a sweep finding on a risk path touches the high-risk surface", api.riskPathTouched("src/services/v2/payments.ts", rules) === true);
  check("one elsewhere does not", api.riskPathTouched("src/utils/strings.ts", rules) === false);
  check("no file is no surface", api.riskPathTouched("", rules) === false);
  check(
    "the sweep derives the flag from the profile's risk paths",
    /touchesHighRiskSurface: riskPathTouched\(f\.file, riskPathRules\(PROFILE\)\),/.test(src),
  );
  check(
    "no hardcoded backend sentence survives in any prompt",
    !/true if it involves prisma\/schema, migrations, auth, payments/.test(src),
  );
  // If these stop being true the field is decorative and this whole fix is
  // pointless, so they are asserted rather than assumed.
  check(
    "the field still gates escalation",
    /finding\.touchesHighRiskSurface &&/.test(src),
  );
  check(
    "…and still chooses Codex's reasoning effort",
    /finding\.touchesHighRiskSurface \? "high" : "medium"/.test(src),
  );
  check(
    "…and still decides whether Gemini is dispatched",
    /geminiHealthy && finding\.touchesHighRiskSurface/.test(src),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall high-risk-surface expectations hold");
