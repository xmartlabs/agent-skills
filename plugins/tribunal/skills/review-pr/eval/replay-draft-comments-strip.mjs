#!/usr/bin/env node
// replay-draft-comments-strip.mjs — the fidelity gate must check the body
// GitHub receives, not the file as drafted.
//
// The defect, raised 2026-09-08 and carried in SKILL.md as a known limit before
// that. The four fidelity checks are substring tests over the report text, and
// the text handed to them still contained the "### Draft comments" section that
// `post-review.mjs` strips on the way out. So an `also at:` location or a
// killReason category drafted only inside that section satisfied the gate and
// was then removed, leaving the posted body without it and the gate reporting
// clean. That is the PR #2082 failure the gate was added to prevent, occurring
// with the gate switched on.
//
// It was worth checking rather than assuming: the first attempt to demonstrate
// it used a one-element `mergedLocations` and got `[]` on both sides, because
// the rule deliberately skips the first location (it is already in the finding
// header). The defect is real, but a malformed fixture would have "proved" it
// either way, which is the whole reason the corpus below is shaped like the
// real field and both directions are asserted.
//
// Two copies of the strip exist by necessity: scripts/strip-draft-comments.mjs
// is the authority for the posting strip, and Workflow scripts cannot import,
// so tribunal.js carries a DRAFT-COMMENTS-STRIP region. This file is the
// contract that keeps them identical — the same one lib/verified.mjs and the
// VERIFIED-GATE region already keep.
//
// Usage: node eval/replay-draft-comments-strip.mjs

import { readFileSync } from "node:fs";
import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW, SKILL_DIR, lib } from "./paths.mjs";
import { stripDraftComments } from "../scripts/strip-draft-comments.mjs";
import { loadChecks, applyFidelityChecks } from "../../../lib/check-report-fidelity.mjs";

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
  loaded = loadRegion(WORKFLOW, "DRAFT-COMMENTS-STRIP", ["stripDraftCommentsFromBody"]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { stripDraftCommentsFromBody } = api;
const src = readFileSync(WORKFLOW, "utf8");

console.log(`\nDRAFT-COMMENTS-STRIP region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

// Shapes taken from what the synthesis prompt actually asks for, plus the ones
// that break naive implementations.
const CORPUS = [
  ["no such section at all", "### 🔴 Blocking\n\n**A** `x.ts:1`\n"],
  [
    "the section last in the file",
    "### 🔴 Blocking\n\n**A** `x.ts:1`\n\n### Draft comments (ready to paste)\n\n- one\n- two\n",
  ],
  [
    "a section followed by another heading",
    "### 🔴 Blocking\n\n**A** `x.ts:1`\n\n### Draft comments\n\n- one\n\n### 🔵 Nits\n\n**B** `y.ts:2`\n",
  ],
  ["the section first, nothing before it", "### Draft comments\n\n- one\n\n### 🔵 Nits\n\n**B**\n"],
  ["an empty section", "### 🔴 Blocking\n\n### Draft comments\n\n### 🔵 Nits\n\n**B**\n"],
  ["a bare heading with no trailing text", "### 🔴 Blocking\n\n### Draft comments\n- one\n"],
  ["an empty document", ""],
  [
    "a lookalike heading that must NOT be stripped",
    "### 🔴 Blocking\n\n### Draft commentary\n\n- keep me\n",
  ],
  [
    "a deeper heading level does not terminate the section",
    "### Draft comments\n\n#### sub\n\n- one\n\n### 🔵 Nits\n\n**B**\n",
  ],
  [
    "two sections, both go",
    "### A\n\n### Draft comments\n\n- one\n\n### B\n\n### Draft comments\n\n- two\n",
  ],
];

console.log(`\n--- the two implementations agree on every shape ---`);
{
  for (const [label, text] of CORPUS) {
    check(
      label,
      stripDraftComments(text) === stripDraftCommentsFromBody(text),
      JSON.stringify({
        authority: stripDraftComments(text),
        region: stripDraftCommentsFromBody(text),
      }).slice(0, 200),
    );
  }
  check(
    "…and the corpus is not vacuous: most shapes really do change",
    CORPUS.filter(([, t]) => stripDraftComments(t) !== t).length >= 6,
  );
  check(
    "the lookalike heading is genuinely kept, by both",
    stripDraftComments(CORPUS[7][1]).includes("keep me") &&
      stripDraftCommentsFromBody(CORPUS[7][1]).includes("keep me"),
  );
  // Written first as "the region tolerates null" and red against correct code:
  // the region had a null guard the authority does not have, so the two
  // disagreed on exactly the input this file exists to keep them agreed on. The
  // guard came out; what is asserted now is that they fail the same way.
  const bothThrow = (v) => {
    const t = (f) => {
      try {
        f(v);
        return "returned";
      } catch {
        return "threw";
      }
    };
    return t(stripDraftComments) === t(stripDraftCommentsFromBody);
  };
  for (const [label, v] of [
    ["null", null],
    ["undefined", undefined],
    ["a number", 7],
  ])
    check(`the two agree on ${label} too, including by failing alike`, bothThrow(v));
}

console.log(`\n--- the gate now fails on what it used to pass ---`);
{
  // The fixture that demonstrated the defect: the extra merged location and the
  // killReason appear ONLY inside the draft-comments section.
  const body = [
    "### 🔴 Blocking",
    "",
    "**Negative amount accepted** `src/invoice.ts:88`",
    "No positive check before create.",
    "",
    "### Draft comments (ready to paste)",
    "",
    "- `src/invoice.ts:88` — also at: src/bulk-invoice.ts:210",
    "- the duplicate above was dismissed as not-triggered",
    "",
  ].join("\n");
  const finding = () => ({
    title: "Negative amount accepted",
    mergedLocations: [
      { file: "src/invoice.ts", line: 88 },
      { file: "src/bulk-invoice.ts", line: 210 },
    ],
    votes: [{ verdict: "REFUTED", killReason: "not-triggered" }],
    mergedFrom: [],
    grounded: { verdict: null, diagnostic: null },
  });

  const checks = loadChecks();
  const asDrafted = applyFidelityChecks([finding()], body, checks)[0];
  const asPosted = applyFidelityChecks([finding()], stripDraftComments(body), checks)[0];

  check(
    "the raw draft still passes — the defect is real, not a mis-specified fixture",
    asDrafted.mergedLocationsMissingFromReport.length === 0 &&
      asDrafted.killReasonMissingFromReport.length === 0,
    JSON.stringify(asDrafted.mergedLocationsMissingFromReport),
  );
  check(
    "the posted body fails on the merged location",
    asPosted.mergedLocationsMissingFromReport.join() === "src/bulk-invoice.ts:210",
    JSON.stringify(asPosted.mergedLocationsMissingFromReport),
  );
  check(
    "…and on the kill reason",
    asPosted.killReasonMissingFromReport.join() === "not-triggered",
  );
  check(
    "one fixture defeats two of the four rules — worth stating plainly",
    asDrafted.mergedLocationsMissingFromReport.length === 0 &&
      asPosted.mergedLocationsMissingFromReport.length === 1 &&
      asDrafted.killReasonMissingFromReport.length === 0 &&
      asPosted.killReasonMissingFromReport.length === 1,
  );

  // And the same body with the items where they belong must still pass, or the
  // fix is just a stricter gate rather than a correct one.
  const good = body.replace(
    "No positive check before create.",
    "No positive check before create. also at: src/bulk-invoice.ts:210. Dismissed as not-triggered.",
  );
  const goodPosted = applyFidelityChecks([finding()], stripDraftComments(good), checks)[0];
  check(
    "a body carrying both items in its findings section still passes",
    goodPosted.mergedLocationsMissingFromReport.length === 0 &&
      goodPosted.killReasonMissingFromReport.length === 0,
    JSON.stringify(goodPosted),
  );
}

console.log(`\n--- wiring: both callers, and the doctrine that describes them ---`);
{
  // Until 2026-09-11 this pinned the exact one-line form. The posted body now
  // also carries the no-pinned-checkout banner, appended in code rather than
  // requested from the synthesis prompt, so the assertion moved from the
  // literal to the two properties it was actually protecting: computed ONCE,
  // and starting from the STRIPPED report. Loosening it to "mentions
  // stripDraftCommentsFromBody somewhere" would have kept it green while a
  // second assignment quietly reintroduced the raw body.
  const postedAssignments = src.match(/const reportAsPosted\s*=/g) || [];
  check(
    "the workflow computes the posted body exactly once, after synthesis",
    postedAssignments.length === 1,
    `found ${postedAssignments.length} assignment(s) of reportAsPosted`,
  );
  check(
    "…and that one starts from the stripped report, not the raw one",
    /const reportAsPosted\s*=\s*(?:\n\s*)?stripDraftCommentsFromBody\(report\)/.test(src),
  );
  check(
    "all four workflow checks run against it, none against the raw report",
    (src.match(/\n\s+reportAsPosted,?\n/g) || []).length >= 3 &&
      !/killReasonMissingFromReportText\(refutedVote\.killReason, report\)/.test(src),
  );
  const cli = readFileSync(lib("check-report-fidelity.mjs"), "utf8");
  check("the CLI loads the strip from the same region", /"DRAFT-COMMENTS-STRIP"/.test(cli));
  check(
    "…and applies it before the checks",
    /applyFidelityChecks\(metrics\.findings, postedText, checks\)/.test(cli),
  );
  const skill = readFileSync(`${SKILL_DIR}/SKILL.md`, "utf8");
  check(
    "SKILL.md no longer carries the limit as known-open",
    !/the file you check still contains the "Draft comments" section/.test(skill),
  );
  check(
    "…and says the gate strips it itself, so nobody is asked to remember",
    /strips the "Draft comments" section itself/.test(skill),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall draft-comments-strip expectations hold");
