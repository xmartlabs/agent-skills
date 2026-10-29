#!/usr/bin/env node
// replay-report-banners.mjs — the REPORT-BANNERS region: which lenses came back
// empty, the banner that names them, and where banners land in the report.
//
// Two defects behind it, both found 2026-10-23. A run on PR #2201 hit the
// session usage limit on 2026-10-21, both lenses came back empty, and the
// Workflow carried on to "0 candidates" and a synthesis that would have read as
// a clean review. And the degraded-run banners were added only to the copy the
// fidelity checks read, so the report the Workflow returns, which is what gets
// posted, never carried them.
//
// The placement case is the one that needs a fixture shaped like the real
// report: when "### Draft comments" is the last section, the posting strip
// removes everything after its heading, so a banner appended at the end goes
// with it. A fixture with another heading after the drafts passes either way.
//
// Usage: node eval/replay-report-banners.mjs

import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";
import { stripDraftComments } from "../scripts/strip-draft-comments.mjs";

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
  loaded = loadRegion(WORKFLOW, "REPORT-BANNERS", [
    "unreturnedLenses",
    "lensesUnreturnedBanner",
    "withBanners",
  ]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { unreturnedLenses, lensesUnreturnedBanner, withBanners } = api;

console.log(`\nREPORT-BANNERS region extracted from ${WORKFLOW}`);
check("region references no run state", violations.length === 0, violations.join(" | "));

console.log("\n--- which lenses came back empty ---");
const plan = [{ focus: "a", files: ["src/a.ts"] }, { focus: "b" }, { focus: "c", files: [] }];
const labels = ["correctness", "security", "lens3"];
const none = unreturnedLenses(plan, [{ findings: [] }, { findings: [] }, { findings: [] }], labels);
check("a lens that returned an empty findings list is not an empty lens", none.length === 0, JSON.stringify(none));
const some = unreturnedLenses(plan, [null, { findings: [] }, undefined], labels);
check(
  "a null or missing answer is, and keeps its files",
  JSON.stringify(some) ===
    JSON.stringify([
      { lens: "correctness", files: ["src/a.ts"] },
      { lens: "lens3", files: [] },
    ]),
  JSON.stringify(some),
);
check(
  "a lens over the whole diff has null files",
  unreturnedLenses([{ focus: "b" }], [null], ["security"])[0].files === null,
);

console.log("\n--- the banner ---");
check("no empty lens, no banner", lensesUnreturnedBanner([], 3) === "");
const banner = lensesUnreturnedBanner(
  [
    { lens: "correctness", files: ["src/a.ts", "src/b.ts"] },
    { lens: "security", files: null },
  ],
  3,
);
check("it counts the empty lenses against the plan", banner.includes("**2 of 3 lens(es) returned nothing**"), banner);
check("it names each lens with its files", banner.includes("  - correctness: `src/a.ts`, `src/b.ts`"), banner);
check("a whole-diff lens reads as the whole diff", banner.includes("  - security: the whole diff"), banner);
check("it says the review cannot approve", /cannot approve the PR/.test(banner), banner);

console.log("\n--- where banners land ---");
const B = "\n\n---\n\n⚠️ **marker banner**";
const draftsLast = "## Review\n\n### Blocking\n1. x\n\n### Draft comments (ready to paste)\n- one\n- two\n";
const placed = withBanners(draftsLast, B);
check("with drafts as the last section, the banner survives the posting strip", stripDraftComments(placed).includes("marker banner"), stripDraftComments(placed));
check("and the drafts are still there for the operator", placed.includes("### Draft comments (ready to paste)\n- one"), placed);
check(
  "a plain append there would have been stripped (the case is live)",
  !stripDraftComments(draftsLast + B).includes("marker banner"),
);
const noDrafts = "## Review\n\n### Blocking\n1. x\n";
check("with no drafts section, the banner is appended", withBanners(noDrafts, B) === noDrafts + B);
check("no banner leaves the report byte for byte", withBanners(draftsLast, "") === draftsLast);

console.log("");
if (failures.length) {
  console.log(`replay-report-banners: ${failures.length} FAILURE(S)`);
  process.exit(1);
}
console.log("replay-report-banners: all checks passed");
