#!/usr/bin/env node
// replay-profile-lookup.mjs — the PROFILE-LOOKUP region of tribunal.js, sliced
// out and run against the frozen client profiles and against broken copies of them.
//
// Every project fact the workflow uses arrives through args.profile, and this
// region is the only gate between a malformed profile and a dispatched lens.
// scripts/load-profile.mjs runs the same region, so it gates the in-context path
// too. Pinned here:
//   · the frozen client profiles validate, each against its own repo.
//   · a missing profile, a profile for another repo, and each malformed field
//     stop the run with a message naming the field; nothing falls back.
//   · the generated bash fragments equal what the client constants produced
//     before they moved (literal goldens, not read back from the profile).
//
// Usage: node eval/replay-profile-lookup.mjs

import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";
import { clientProfileByReviewer, loadClientProfiles } from "./client-profiles.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { api, violations } = loadRegion(WORKFLOW, "PROFILE-LOOKUP", [
  "validateProfile",
  "deployBranchCasePattern",
  "deployBranchList",
  "crossRepoFlags",
]);
const PROFILES = loadClientProfiles();
const BE = clientProfileByReviewer("backend").profile.repo;
const FE = clientProfileByReviewer("frontend").profile.repo;
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");

const refusal = (fn) => {
  try {
    fn();
    return "";
  } catch (err) {
    return err.message;
  }
};
const edited = (repo, edit) => {
  const p = JSON.parse(JSON.stringify(PROFILES[repo]));
  edit(p);
  return p;
};

console.log(`\nprofile lookup — PROFILE-LOOKUP region of tribunal.js`);
check("the region is pure (no run state, no dispatch)", violations.length === 0, violations.join("; "));
check("both frozen profiles are present", BE in PROFILES && FE in PROFILES, Object.keys(PROFILES).join(", "));
for (const repo of [BE, FE]) {
  check(`${repo}'s profile validates against its own repo`, refusal(() => api.validateProfile(PROFILES[repo], repo)) === "");
}
check("a JSON string is accepted as well as an object", refusal(() => api.validateProfile(JSON.stringify(PROFILES[BE]), BE)) === "");
check("a run with no repo argument takes the profile's repo", refusal(() => api.validateProfile(PROFILES[FE], null)) === "");

console.log(`\n--- refusals name what is wrong ---`);
const CASES = [
  ["a missing profile", () => api.validateProfile(undefined, BE), /args\.profile is missing/],
  ["a profile for another repo", () => api.validateProfile(PROFILES[BE], FE), new RegExp(`describes ${esc(BE)}, but this run reviews ${esc(FE)}`)],
  ["a wrong schemaVersion", () => api.validateProfile(edited(BE, (p) => (p.schemaVersion = 2)), BE), /schemaVersion must be 1/],
  ["an unknown reviewer kind", () => api.validateProfile(edited(BE, (p) => (p.reviewer = "tribunal:code-review-expert")), BE), /reviewer must be one of backend, frontend/],
  ["a deploy branch with shell metacharacters", () => api.validateProfile(edited(BE, (p) => (p.deployBranches = ["staging; rm -rf ~"])), BE), /deployBranches must be an array of branch names/],
  ["a missing required focus", () => api.validateProfile(edited(FE, (p) => delete p.foci.security), FE), /foci\.security must be a non-empty sentence/],
  ["an unknown focus key", () => api.validateProfile(edited(FE, (p) => (p.foci.style = "naming")), FE), /foci\.style is not a known focus key/],
  ["one sentence under two keys", () => api.validateProfile(edited(FE, (p) => (p.foci.performance = p.foci.security)), FE), /FOCI_LABEL could not attribute it/],
  ["an empty high-risk sentence", () => api.validateProfile(edited(BE, (p) => (p.highRiskSurface = " ")), BE), /highRiskSurface must be a non-empty sentence/],
  ["a cross-repo anchor naming the repo itself", () => api.validateProfile(edited(BE, (p) => (p.crossRepoAnchors = [{ repo: BE, ref: "development" }])), BE), /crossRepoAnchors must be an array/],
];
for (const [label, fn, re] of CASES) {
  const msg = refusal(fn);
  check(`${label} is refused`, re.test(msg), msg || "accepted");
}
{
  const msg = refusal(() => api.validateProfile(edited(BE, (p) => {
    p.reviewer = "x";
    p.highRiskSurface = "";
  }), BE));
  check("every error is reported at once, not just the first", /reviewer/.test(msg) && /highRiskSurface/.test(msg), msg);
}

console.log(`\n--- generated fragments equal the constants they replaced ---`);
for (const repo of [BE, FE]) {
  check(`${repo}: deploy case pattern`, api.deployBranchCasePattern(PROFILES[repo]) === "staging|uat|production", api.deployBranchCasePattern(PROFILES[repo]));
  check(`${repo}: deploy list`, api.deployBranchList(PROFILES[repo]) === "staging,uat,production", api.deployBranchList(PROFILES[repo]));
}
check("BE cross-repo flag", api.crossRepoFlags(PROFILES[BE]) === ` --cross-repo "${FE}@development"`, api.crossRepoFlags(PROFILES[BE]));
check("FE cross-repo flag", api.crossRepoFlags(PROFILES[FE]) === ` --cross-repo "${BE}@development"`, api.crossRepoFlags(PROFILES[FE]));
check(
  "a repo with no deploy branches gets a pattern no branch can match",
  api.deployBranchCasePattern(edited(BE, (p) => (p.deployBranches = []))) === "__tribunal_no_deploy_branch__",
);
check("a repo with no siblings gets no cross-repo flag", api.crossRepoFlags(edited(BE, (p) => (p.crossRepoAnchors = []))) === "");

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall profile-lookup expectations hold");
